//! A pure Rust embedded execution kernel powered by Rhai.
//!
//! Unlike Python/Node socket daemons, this kernel executes entirely in-process
//! without spawning child processes or managing Unix domain sockets, while
//! preserving variable state across separate CLI invocations, capturing print
//! output, enforcing execution limits, and supporting snapshot/restore.
//!
//! Built-in helper functions (`head`, `tail`, `slice_lines`, `outline`, `load`,
//! `grep`, `read`, `write`, `edit`, `sh`, `parse_json`, `to_json`) allow
//! agents to explore codebases, inspect data, and edit files without flooding context.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use glob::glob;
use rhai::{Array, Dynamic, Engine, Map, Scope};
use serde_json::Value;
use umoja_domain::error::{DomainError, Result};
use umoja_domain::kernel::{ExecOutcome, ExecRequest, KernelLanguage, KernelStatus, VarSummary};
use umoja_domain::ports::KernelPort;

use crate::paths::Paths;

#[derive(Default)]
struct SessionState {
    scope: Scope<'static>,
    history: Vec<String>,
    initialized: bool,
}

#[derive(Clone)]
pub struct RhaiKernel {
    paths: Option<Paths>,
    sessions: Arc<RwLock<HashMap<String, Arc<Mutex<SessionState>>>>>,
    max_operations: Option<u64>,
}

fn register_builtins(engine: &mut Engine) {
    // -------------------------------------------------------------------------
    // head & tail
    // -------------------------------------------------------------------------
    engine.register_fn("head", |path: &str| -> String {
        head_lines(path, 50)
    });
    engine.register_fn("head", |path: &str, lines: i64| -> String {
        head_lines(path, lines.max(0) as usize)
    });

    engine.register_fn("tail", |path: &str| -> String {
        tail_lines(path, 50)
    });
    engine.register_fn("tail", |path: &str, lines: i64| -> String {
        tail_lines(path, lines.max(0) as usize)
    });

    // -------------------------------------------------------------------------
    // slice_lines
    // -------------------------------------------------------------------------
    engine.register_fn("slice_lines", |path: &str, start: i64, end: i64| -> String {
        slice_file_lines(path, start.max(1) as usize, end.max(1) as usize)
    });

    // -------------------------------------------------------------------------
    // outline
    // -------------------------------------------------------------------------
    engine.register_fn("outline", |path: &str| -> String {
        outline_file(path)
    });

    // -------------------------------------------------------------------------
    // read, write, edit
    // -------------------------------------------------------------------------
    engine.register_fn("read", |path: &str| -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| format!("Error reading {path}: {e}"))
    });

    engine.register_fn("write", |path: &str, content: &str| -> bool {
        if let Some(parent) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(path, content).is_ok()
    });

    engine.register_fn("edit", |path: &str, old_text: &str, new_text: &str| -> bool {
        if let Ok(content) = std::fs::read_to_string(path) {
            if content.contains(old_text) {
                let replaced = content.replacen(old_text, new_text, 1);
                return std::fs::write(path, replaced).is_ok();
            }
        }
        false
    });

    // -------------------------------------------------------------------------
    // load
    // -------------------------------------------------------------------------
    engine.register_fn("load", |pattern: &str| -> Dynamic {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let mut results = Array::new();
            if let Ok(paths) = glob(pattern) {
                for entry in paths.flatten() {
                    if entry.is_file() {
                        let path_str = entry.display().to_string();
                        if let Ok(content) = std::fs::read_to_string(&entry) {
                            let line_count = content.lines().count() as i64;
                            let size = content.len() as i64;
                            let mut file_map = Map::new();
                            file_map.insert("path".into(), Dynamic::from(path_str));
                            file_map.insert("content".into(), Dynamic::from(content));
                            file_map.insert("lines".into(), Dynamic::from(line_count));
                            file_map.insert("size".into(), Dynamic::from(size));
                            results.push(Dynamic::from(file_map));
                        }
                    }
                }
            }
            Dynamic::from(results)
        } else {
            let path = Path::new(pattern);
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if let Ok(val) = serde_json::from_str::<Value>(&content) {
                        if let Ok(dyn_val) = rhai::serde::to_dynamic(&val) {
                            return dyn_val;
                        }
                    }
                }
            }
            if let Ok(content) = std::fs::read_to_string(path) {
                Dynamic::from(content)
            } else {
                Dynamic::from(format!("Error: could not read file '{pattern}'"))
            }
        }
    });

    // -------------------------------------------------------------------------
    // grep
    // -------------------------------------------------------------------------
    engine.register_fn("grep", |pattern: &str| -> Dynamic {
        grep_path(".", pattern)
    });

    engine.register_fn("grep", |pattern: &str, target: &str| -> Dynamic {
        grep_path(target, pattern)
    });

    engine.register_fn("grep", |files: Array, pattern: &str| -> Array {
        grep_array(&files, pattern)
    });

    // Method syntax: files.grep("pattern")
    engine.register_fn("grep", |files: &mut Array, pattern: &str| -> Array {
        grep_array(files, pattern)
    });

    // -------------------------------------------------------------------------
    // sh
    // -------------------------------------------------------------------------
    engine.register_fn("sh", |cmd: &str| -> String {
        let output = Command::new("sh").arg("-c").arg(cmd).output();
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                if !stdout.is_empty() {
                    stdout.trim_end().to_string()
                } else {
                    String::from_utf8_lossy(&out.stderr).trim_end().to_string()
                }
            }
            Err(e) => format!("sh error: {e}"),
        }
    });

    // -------------------------------------------------------------------------
    // JSON parsing & stringification
    // -------------------------------------------------------------------------
    engine.register_fn("parse_json", |s: &str| -> Dynamic {
        serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| rhai::serde::to_dynamic(&v).ok())
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("json_parse", |s: &str| -> Dynamic {
        serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| rhai::serde::to_dynamic(&v).ok())
            .unwrap_or(Dynamic::UNIT)
    });

    engine.register_fn("to_json", |val: Dynamic| -> String {
        rhai::serde::from_dynamic::<Value>(&val)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| val.to_string())
    });
    engine.register_fn("json_stringify", |val: Dynamic| -> String {
        rhai::serde::from_dynamic::<Value>(&val)
            .ok()
            .and_then(|v| serde_json::to_string(&v).ok())
            .unwrap_or_else(|| val.to_string())
    });

    // -------------------------------------------------------------------------
    // Native fast dataset helpers
    // -------------------------------------------------------------------------
    engine.register_fn("count_lines", |files: &mut Array| -> i64 {
        let mut total = 0i64;
        for item in files.iter() {
            if let Some(map) = item.clone().try_cast::<Map>() {
                if let Some(lines) = map.get("lines") {
                    if let Ok(l) = lines.as_int() {
                        total += l;
                    }
                }
            }
        }
        total
    });

    engine.register_fn("total_size", |files: &mut Array| -> i64 {
        let mut total = 0i64;
        for item in files.iter() {
            if let Some(map) = item.clone().try_cast::<Map>() {
                if let Some(size) = map.get("size") {
                    if let Ok(s) = size.as_int() {
                        total += s;
                    }
                }
            }
        }
        total
    });

    engine.register_fn("paths", |files: &mut Array| -> Array {
        let mut result = Array::new();
        for item in files.iter() {
            if let Some(map) = item.clone().try_cast::<Map>() {
                if let Some(p) = map.get("path") {
                    result.push(p.clone());
                }
            }
        }
        result
    });

    engine.register_fn("filter_by_content", |files: &mut Array, pattern: &str| -> Array {
        let mut result = Array::new();
        let pat_lower = pattern.to_lowercase();
        for item in files.iter() {
            if let Some(map) = item.clone().try_cast::<Map>() {
                if let Some(content) = map.get("content") {
                    let s = content.to_string();
                    if s.to_lowercase().contains(&pat_lower) {
                        result.push(item.clone());
                    }
                }
            }
        }
        result
    });
}

fn head_lines(path: &str, n: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => content
            .lines()
            .take(n)
            .collect::<Vec<&str>>()
            .join("\n"),
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn tail_lines(path: &str, n: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(n);
            lines[start..].join("\n")
        }
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn slice_file_lines(path: &str, start: usize, end: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            if lines.is_empty() {
                return String::new();
            }
            let s = (start.max(1) - 1).min(lines.len().saturating_sub(1));
            let e = end.min(lines.len());
            if s < e {
                lines[s..e].join("\n")
            } else {
                String::new()
            }
        }
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn outline_file(path: &str) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let mut matches = Vec::new();
            for (idx, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub struct ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("pub enum ")
                    || trimmed.starts_with("enum ")
                    || trimmed.starts_with("pub trait ")
                    || trimmed.starts_with("trait ")
                    || trimmed.starts_with("impl ")
                    || trimmed.starts_with("pub mod ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("def ")
                    || trimmed.starts_with('#')
                {
                    matches.push(format!("{}: {}", idx + 1, trimmed));
                }
            }
            matches.join("\n")
        }
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn is_ignored_path(path: &Path) -> bool {
    for comp in path.components() {
        if let std::path::Component::Normal(os_str) = comp {
            let s = os_str.to_string_lossy();
            if s == "target" || s == ".git" || s == "node_modules" || s == ".cargo" || s == "vendor" {
                return true;
            }
        }
    }
    false
}

fn grep_path(target: &str, pattern: &str) -> Dynamic {
    let mut matches = Array::new();
    let pat_lower = pattern.to_lowercase();

    let glob_pat = if target == "." {
        "**/*".to_string()
    } else if target.contains('*') {
        target.to_string()
    } else if Path::new(target).is_file() {
        if let Ok(content) = std::fs::read_to_string(target) {
            for (idx, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&pat_lower) {
                    let mut m = Map::new();
                    m.insert("path".into(), Dynamic::from(target.to_string()));
                    m.insert("line".into(), Dynamic::from((idx + 1) as i64));
                    m.insert("content".into(), Dynamic::from(line.trim().to_string()));
                    matches.push(Dynamic::from(m));
                }
            }
        }
        return Dynamic::from(matches);
    } else {
        format!("{target}/**/*")
    };

    if let Ok(paths) = glob(&glob_pat) {
        for entry in paths.flatten() {
            if entry.is_file() && !is_ignored_path(&entry) {
                if let Ok(content) = std::fs::read_to_string(&entry) {
                    let path_str = entry.display().to_string();
                    for (idx, line) in content.lines().enumerate() {
                        if line.to_lowercase().contains(&pat_lower) {
                            let mut m = Map::new();
                            m.insert("path".into(), Dynamic::from(path_str.clone()));
                            m.insert("line".into(), Dynamic::from((idx + 1) as i64));
                            m.insert("content".into(), Dynamic::from(line.trim().to_string()));
                            matches.push(Dynamic::from(m));
                            if matches.len() >= 200 {
                                return Dynamic::from(matches);
                            }
                        }
                    }
                }
            }
        }
    }

    Dynamic::from(matches)
}

fn grep_array(files: &[Dynamic], pattern: &str) -> Array {
    let mut matches = Array::new();
    let pat_lower = pattern.to_lowercase();

    for item in files {
        if let Ok(map) = item.clone().into_typed_array::<Map>() {
            for m in map {
                if let (Some(path_dyn), Some(content_dyn)) = (m.get("path"), m.get("content")) {
                    let path_str = path_dyn.to_string();
                    let content_str = content_dyn.to_string();
                    for (idx, line) in content_str.lines().enumerate() {
                        if line.to_lowercase().contains(&pat_lower) {
                            let mut hit = Map::new();
                            hit.insert("path".into(), Dynamic::from(path_str.clone()));
                            hit.insert("line".into(), Dynamic::from((idx + 1) as i64));
                            hit.insert("content".into(), Dynamic::from(line.trim().to_string()));
                            matches.push(Dynamic::from(hit));
                        }
                    }
                }
            }
        } else if let Some(m) = item.clone().try_cast::<Map>() {
            if let (Some(path_dyn), Some(content_dyn)) = (m.get("path"), m.get("content")) {
                let path_str = path_dyn.to_string();
                let content_str = content_dyn.to_string();
                for (idx, line) in content_str.lines().enumerate() {
                    if line.to_lowercase().contains(&pat_lower) {
                        let mut hit = Map::new();
                        hit.insert("path".into(), Dynamic::from(path_str.clone()));
                        hit.insert("line".into(), Dynamic::from((idx + 1) as i64));
                        hit.insert("content".into(), Dynamic::from(line.trim().to_string()));
                        matches.push(Dynamic::from(hit));
                    }
                }
            }
        } else if let Ok(s) = item.clone().into_string() {
            if s.to_lowercase().contains(&pat_lower) {
                matches.push(Dynamic::from(s));
            }
        }
    }

    matches
}

impl RhaiKernel {
    pub fn new() -> Self {
        let max_operations = std::env::var("UMOJA_MAX_OPERATIONS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .or(Some(20_000_000));
        Self {
            paths: None,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            max_operations,
        }
    }

    pub fn with_paths(mut self, paths: Paths) -> Self {
        self.paths = Some(paths);
        self
    }

    pub fn with_operations_limit(mut self, limit: Option<u64>) -> Self {
        self.max_operations = limit;
        self
    }

    fn create_engine(&self) -> Engine {
        let mut engine = Engine::new();
        engine.set_optimization_level(rhai::OptimizationLevel::Full);
        if let Some(limit) = self.max_operations {
            engine.set_max_operations(limit);
        }
        register_builtins(&mut engine);
        engine
    }

    fn history_file(&self, session_id: &str) -> Option<PathBuf> {
        self.paths
            .as_ref()
            .map(|p| p.artifacts(session_id).join("rhai_history.jsonl"))
    }

    fn scope_file(&self, session_id: &str) -> Option<PathBuf> {
        self.paths
            .as_ref()
            .map(|p| p.artifacts(session_id).join("rhai_scope.json"))
    }

    fn load_history_if_needed(&self, session_id: &str, state: &mut SessionState) {
        if state.initialized {
            return;
        }
        state.initialized = true;

        if let Some(scope_path) = self.scope_file(session_id) {
            if scope_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&scope_path) {
                    if let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(&content) {
                        for (k, v) in map {
                            if let Ok(dyn_val) = rhai::serde::to_dynamic(&v) {
                                state.scope.push(k, dyn_val);
                            }
                        }
                    }
                }
            }
        }

        if let Some(hist_path) = self.history_file(session_id) {
            if hist_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&hist_path) {
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            if let Ok(code) = serde_json::from_str::<String>(trimmed) {
                                state.history.push(code);
                            }
                        }
                    }
                }
            }
        }
    }

    fn persist_session_state(&self, session_id: &str, code: &str, scope: &Scope) {
        if let Some(path) = self.history_file(session_id) {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Ok(json_line) = serde_json::to_string(code) {
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                {
                    let _ = writeln!(file, "{json_line}");
                }
            }
        }

        if let Some(scope_path) = self.scope_file(session_id) {
            let mut map = HashMap::new();
            for (name, _is_const, val) in scope.iter() {
                if let Ok(json_val) = rhai::serde::from_dynamic::<Value>(&val) {
                    map.insert(name.to_string(), json_val);
                }
            }
            if let Ok(json_str) = serde_json::to_string(&map) {
                let _ = std::fs::write(scope_path, json_str);
            }
        }
    }

    fn get_or_create_session(&self, session_id: &str) -> Arc<Mutex<SessionState>> {
        let mut map = self.sessions.write().expect("lock poisoned");
        let session = map
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(SessionState::default())))
            .clone();

        let mut lock = session.lock().expect("lock poisoned");
        self.load_history_if_needed(session_id, &mut lock);
        drop(lock);

        session
    }
}

impl Default for RhaiKernel {
    fn default() -> Self {
        Self::new()
    }
}

impl KernelPort for RhaiKernel {
    fn language(&self) -> KernelLanguage {
        KernelLanguage::Rhai
    }

    fn status(&self, session_id: &str) -> Result<KernelStatus> {
        let map = self.sessions.read().expect("lock poisoned");
        if map.contains_key(session_id) {
            Ok(KernelStatus::Ready)
        } else if let Some(path) = self.history_file(session_id) {
            if path.exists() {
                Ok(KernelStatus::Ready)
            } else {
                Ok(KernelStatus::Cold)
            }
        } else {
            Ok(KernelStatus::Cold)
        }
    }

    fn ensure(&self, session_id: &str) -> Result<KernelStatus> {
        let _ = self.get_or_create_session(session_id);
        Ok(KernelStatus::Ready)
    }

    fn execute(&self, request: &ExecRequest) -> Result<ExecOutcome> {
        let session = self.get_or_create_session(&request.session_id);
        let mut session_lock = session.lock().expect("session lock poisoned");

        let mut engine = self.create_engine();

        let stdout_buffer = Arc::new(Mutex::new(Vec::<String>::new()));
        let stdout_clone = Arc::clone(&stdout_buffer);

        engine.on_print(move |s| {
            if let Ok(mut lock) = stdout_clone.lock() {
                lock.push(s.to_string());
            }
        });

        let start = Instant::now();
        let eval_res = engine.eval_with_scope::<Dynamic>(&mut session_lock.scope, &request.code);
        let duration = start.elapsed();

        let stdout = stdout_buffer
            .lock()
            .map(|lines| lines.join("\n"))
            .unwrap_or_default();

        match eval_res {
            Ok(val) => {
                session_lock.history.push(request.code.clone());
                self.persist_session_state(&request.session_id, &request.code, &session_lock.scope);
                drop(session_lock);

                let result_str = if val.is_unit() {
                    None
                } else {
                    Some(val.to_string())
                };

                let outcome = ExecOutcome {
                    ok: true,
                    stdout,
                    stderr: String::new(),
                    result: result_str,
                    error: None,
                    timed_out: false,
                    duration_ms: duration.as_millis() as u64,
                    truncated_bytes: 0,
                };
                Ok(outcome.clip(request.max_output_bytes))
            }
            Err(err) => {
                let err_str = err.to_string();
                let timed_out = err_str.contains("Too many operations") || err_str.contains("Operation timeout");
                let outcome = ExecOutcome {
                    ok: false,
                    stdout,
                    stderr: String::new(),
                    result: None,
                    error: Some(err_str),
                    timed_out,
                    duration_ms: duration.as_millis() as u64,
                    truncated_bytes: 0,
                };
                Ok(outcome.clip(request.max_output_bytes))
            }
        }
    }

    fn vars(&self, session_id: &str) -> Result<Vec<VarSummary>> {
        let session = self.get_or_create_session(session_id);
        let session_lock = session.lock().expect("session lock poisoned");

        let mut vars = Vec::new();
        for (name, _is_const, val) in session_lock.scope.iter() {
            let type_name = val.type_name().to_string();
            let mut summary = VarSummary {
                name: name.to_string(),
                type_name,
                length: None,
                size_bytes: None,
                preview: None,
            };

            if val.is_array() {
                if let Ok(arr) = val.clone().into_typed_array::<Dynamic>() {
                    summary.length = Some(arr.len() as u64);
                }
            } else if val.is_map() {
                if let Ok(map) = val.clone().into_typed_array::<Map>() {
                    summary.length = Some(map.len() as u64);
                }
            } else if val.is_string() {
                if let Ok(s) = val.clone().into_string() {
                    summary.length = Some(s.len() as u64);
                }
            }

            vars.push(summary);
        }

        Ok(vars)
    }

    fn reset(&self, session_id: &str) -> Result<()> {
        let session = self.get_or_create_session(session_id);
        let mut session_lock = session.lock().expect("session lock poisoned");
        session_lock.scope.clear();
        session_lock.history.clear();
        if let Some(path) = self.history_file(session_id) {
            let _ = std::fs::remove_file(path);
        }
        if let Some(path) = self.scope_file(session_id) {
            let _ = std::fs::remove_file(path);
        }
        Ok(())
    }

    fn shutdown(&self, session_id: &str) -> Result<()> {
        let mut map = self.sessions.write().expect("lock poisoned");
        map.remove(session_id);
        Ok(())
    }

    fn snapshot(&self, session_id: &str) -> Result<Option<String>> {
        let session = self.get_or_create_session(session_id);
        let session_lock = session.lock().expect("session lock poisoned");
        let encoded = serde_json::to_string(&session_lock.history)
            .map_err(|e| DomainError::adapter("snapshot serialization", e))?;
        Ok(Some(encoded))
    }

    fn restore(&self, session_id: &str) -> Result<bool> {
        let session = self.get_or_create_session(session_id);
        let mut session_lock = session.lock().expect("session lock poisoned");
        if let Some(path) = self.history_file(session_id) {
            if path.exists() {
                session_lock.scope.clear();
                session_lock.history.clear();
                session_lock.initialized = false;
                self.load_history_if_needed(session_id, &mut session_lock);
                return Ok(true);
            }
        }
        Ok(false)
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_persist_between_evaluations() {
        let kernel = RhaiKernel::new();
        let outcome1 = kernel
            .execute(&ExecRequest::new("ses-1", "let x = 40 + 2;").unwrap())
            .unwrap();
        assert!(outcome1.ok);

        let outcome2 = kernel
            .execute(&ExecRequest::new("ses-1", "x + 1").unwrap())
            .unwrap();
        assert!(outcome2.ok);
        assert_eq!(outcome2.result.as_deref(), Some("43"));
    }

    #[test]
    fn stdout_print_and_return_values_are_separated() {
        let kernel = RhaiKernel::new();
        let outcome = kernel
            .execute(&ExecRequest::new("ses-1", "print(\"hello rhai\"); 10 * 10").unwrap())
            .unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.stdout.trim(), "hello rhai");
        assert_eq!(outcome.result.as_deref(), Some("100"));
    }

    #[test]
    fn execution_failure_does_not_corrupt_existing_scope() {
        let kernel = RhaiKernel::new();
        kernel
            .execute(&ExecRequest::new("ses-1", "let stable = 99;").unwrap())
            .unwrap();

        let broken = kernel
            .execute(&ExecRequest::new("ses-1", "throw \"boom\";").unwrap())
            .unwrap();
        assert!(!broken.ok);

        let after = kernel
            .execute(&ExecRequest::new("ses-1", "stable").unwrap())
            .unwrap();
        assert_eq!(after.result.as_deref(), Some("99"));
    }

    #[test]
    fn vars_inspection_lists_all_bindings() {
        let kernel = RhaiKernel::new();
        kernel
            .execute(&ExecRequest::new("ses-1", "let numbers = [1, 2, 3]; let name = \"umoja\";").unwrap())
            .unwrap();

        let vars = kernel.vars("ses-1").unwrap();
        assert!(vars.iter().any(|v| v.name == "numbers"));
        assert!(vars.iter().any(|v| v.name == "name"));
    }

    #[test]
    fn reset_clears_scope() {
        let kernel = RhaiKernel::new();
        kernel
            .execute(&ExecRequest::new("ses-1", "let to_delete = 1;").unwrap())
            .unwrap();

        kernel.reset("ses-1").unwrap();
        let vars = kernel.vars("ses-1").unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn runaway_loop_exceeds_max_operations_and_reports_timeout() {
        let kernel = RhaiKernel::new().with_operations_limit(Some(500));
        let outcome = kernel
            .execute(&ExecRequest::new("ses-1", "loop {}").unwrap())
            .unwrap();
        assert!(!outcome.ok);
        assert!(outcome.timed_out);
    }

    #[test]
    fn helper_functions_load_head_and_grep() {
        let kernel = RhaiKernel::new();
        let outcome = kernel
            .execute(&ExecRequest::new("ses-1", "let files = load(\"src/**/*.rs\"); files.len()").unwrap())
            .unwrap();
        assert!(outcome.ok, "exec failed: {:?}", outcome.error);
        let count: i64 = outcome.result.unwrap().parse().unwrap();
        assert!(count > 0);

        let grep_outcome = kernel
            .execute(&ExecRequest::new("ses-1", "let matches = grep(\"RhaiKernel\", \"src/**/*.rs\"); matches.len()").unwrap())
            .unwrap();
        assert!(grep_outcome.ok);
        let match_count: i64 = grep_outcome.result.unwrap().parse().unwrap();
        assert!(match_count > 0);
    }
}
