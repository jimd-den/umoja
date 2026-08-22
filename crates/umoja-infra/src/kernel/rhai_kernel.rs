//! A pure Rust embedded execution kernel powered by Rhai.
//!
//! Executes entirely in-process without spawning child processes or managing
//! Unix domain sockets, preserving variable state across separate CLI invocations,
//! capturing stdout output, enforcing execution limits, and supporting snapshot/restore.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use rhai::{Dynamic, Engine, Map, Scope};
use serde_json::Value;
use umoja_domain::error::{DomainError, Result};
use umoja_domain::kernel::{ExecOutcome, ExecRequest, KernelLanguage, KernelStatus, VarSummary};
use umoja_domain::ports::KernelPort;

use super::builtins;
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

pub fn preprocess_raw_strings(input: &str) -> String {
    let mut out = String::with_capacity(input.len() + 16);
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == 'r' && i + 1 < len && (chars[i + 1] == '"' || chars[i + 1] == '#') {
            let mut h_count = 0;
            let mut j = i + 1;
            while j < len && chars[j] == '#' {
                h_count += 1;
                j += 1;
            }

            if j < len && chars[j] == '"' {
                let content_start = j + 1;
                let mut content_end = None;
                let mut k = content_start;

                while k < len {
                    if chars[k] == '"' {
                        let mut match_hashes = true;
                        if k + h_count < len {
                            for h in 0..h_count {
                                if chars[k + 1 + h] != '#' {
                                    match_hashes = false;
                                    break;
                                }
                            }
                        } else {
                            match_hashes = false;
                        }

                        if match_hashes {
                            content_end = Some(k);
                            break;
                        }
                    }
                    k += 1;
                }

                if let Some(end_idx) = content_end {
                    let raw_str: String = chars[content_start..end_idx].iter().collect();
                    if let Ok(escaped) = serde_json::to_string(&raw_str) {
                        out.push_str(&escaped);
                        i = end_idx + 1 + h_count;
                        continue;
                    }
                }
            }
        }

        out.push(chars[i]);
        i += 1;
    }

    out
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
        builtins::register_all(&mut engine);
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

        let preprocessed_code = preprocess_raw_strings(&request.code);

        let start = Instant::now();
        let eval_res = engine.eval_with_scope::<Dynamic>(&mut session_lock.scope, &preprocessed_code);
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
        if let Some(path) = self.scope_file(session_id) {
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

    #[test]
    fn raw_string_literals_evaluate_with_unescaped_quotes_and_backslashes() {
        let kernel = RhaiKernel::new();
        let code = r##"
            let raw = r#"match esc { '\\' | '"' | '\'' => true, _ => false }"#;
            raw.contains('\\') && raw.contains('"')
        "##;
        let outcome = kernel.execute(&ExecRequest::new("ses-1", code).unwrap()).unwrap();
        assert!(outcome.ok, "error: {:?}", outcome.error);
        assert_eq!(outcome.result.as_deref(), Some("true"));
    }
}
