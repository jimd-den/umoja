//! A pure Rust embedded execution kernel powered by Rhai.
//!
//! Unlike Python/Node socket daemons, this kernel executes entirely in-process
//! without spawning child processes or managing Unix domain sockets, while
//! preserving variable state across separate CLI invocations, capturing print
//! output, enforcing execution limits, and supporting snapshot/restore.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use umoja_domain::error::{DomainError, Result};
use umoja_domain::kernel::{ExecOutcome, ExecRequest, KernelLanguage, KernelStatus, VarSummary};
use umoja_domain::ports::KernelPort;
use rhai::{Dynamic, Engine, Scope};

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

impl RhaiKernel {
    pub fn new() -> Self {
        Self {
            paths: None,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            max_operations: Some(1_000_000),
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

    fn history_file(&self, session_id: &str) -> Option<PathBuf> {
        self.paths
            .as_ref()
            .map(|p| p.artifacts(session_id).join("rhai_history.jsonl"))
    }

    fn load_history_if_needed(&self, session_id: &str, state: &mut SessionState) {
        if state.initialized {
            return;
        }
        state.initialized = true;

        if let Some(path) = self.history_file(session_id) {
            if path.exists() {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let mut engine = Engine::new();
                    if let Some(limit) = self.max_operations {
                        engine.set_max_operations(limit);
                    }
                    for line in content.lines() {
                        let trimmed = line.trim();
                        if !trimmed.is_empty() {
                            if let Ok(code) = serde_json::from_str::<String>(trimmed) {
                                let _ = engine.eval_with_scope::<Dynamic>(&mut state.scope, &code);
                                state.history.push(code);
                            }
                        }
                    }
                }
            }
        }
    }

    fn persist_history_entry(&self, session_id: &str, code: &str) {
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

        let mut engine = Engine::new();
        if let Some(limit) = self.max_operations {
            engine.set_max_operations(limit);
        }

        let stdout_buffer = Arc::new(Mutex::new(Vec::<String>::new()));
        let stdout_clone = Arc::clone(&stdout_buffer);
        engine.on_print(move |s| {
            stdout_clone.lock().expect("stdout lock").push(s.to_string());
        });

        let debug_clone = Arc::clone(&stdout_buffer);
        engine.on_debug(move |s, _src, _pos| {
            debug_clone.lock().expect("stdout lock").push(s.to_string());
        });

        let start = Instant::now();
        let mut working_scope = session_lock.scope.clone();

        let eval_result: std::result::Result<Dynamic, Box<rhai::EvalAltResult>> =
            engine.eval_with_scope(&mut working_scope, &request.code);

        let duration_ms = start.elapsed().as_millis().max(1) as u64;
        let stdout_lines = stdout_buffer.lock().expect("stdout lock");
        let stdout_text = if stdout_lines.is_empty() {
            String::new()
        } else {
            let mut out = stdout_lines.join("\n");
            out.push('\n');
            out
        };

        match eval_result {
            Ok(dynamic_val) => {
                session_lock.scope = working_scope;
                session_lock.history.push(request.code.clone());
                self.persist_history_entry(&request.session_id, &request.code);

                let result_str = if dynamic_val.is_unit() {
                    None
                } else {
                    Some(dynamic_val.to_string())
                };

                let outcome = ExecOutcome {
                    ok: true,
                    stdout: stdout_text,
                    stderr: String::new(),
                    result: result_str,
                    error: None,
                    duration_ms,
                    truncated_bytes: 0,
                    timed_out: false,
                };
                Ok(outcome.clip(request.max_output_bytes))
            }
            Err(err) => {
                let err_str = err.to_string();
                let timed_out = err_str.contains("Too many operations")
                    || err_str.contains("Exceeded maximum operations");

                let outcome = ExecOutcome {
                    ok: false,
                    stdout: stdout_text,
                    stderr: err_str.clone(),
                    result: None,
                    error: Some(err_str),
                    duration_ms,
                    truncated_bytes: 0,
                    timed_out,
                };
                Ok(outcome.clip(request.max_output_bytes))
            }
        }
    }

    fn vars(&self, session_id: &str) -> Result<Vec<VarSummary>> {
        let session = self.get_or_create_session(session_id);
        let session_lock = session.lock().expect("session lock poisoned");

        let mut summaries = Vec::new();
        for (name, _is_const, val) in session_lock.scope.iter() {
            let preview_str = format!("{val}");
            let length = if val.is_array() {
                val.clone().try_cast::<rhai::Array>().map(|a| a.len() as u64)
            } else if val.is_map() {
                val.clone().try_cast::<rhai::Map>().map(|m| m.len() as u64)
            } else if val.is_string() {
                val.clone().try_cast::<rhai::ImmutableString>().map(|s| s.len() as u64)
            } else {
                None
            };

            summaries.push(VarSummary {
                name: name.to_string(),
                type_name: val.type_name().to_string(),
                length,
                size_bytes: Some(preview_str.len() as u64),
                preview: Some(preview_str),
            });
        }
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(summaries)
    }

    fn reset(&self, session_id: &str) -> Result<()> {
        let session = self.get_or_create_session(session_id);
        let mut session_lock = session.lock().expect("session lock poisoned");
        session_lock.scope.clear();
        session_lock.history.clear();
        if let Some(path) = self.history_file(session_id) {
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

        let mut vars = HashMap::new();
        for (name, _, val) in session_lock.scope.iter() {
            vars.insert(name.to_string(), val.to_string());
        }

        let json = serde_json::to_string(&vars)
            .map_err(|e| DomainError::Invalid(format!("snapshot serialisation failed: {e}")))?;
        Ok(Some(json))
    }

    fn restore(&self, session_id: &str) -> Result<bool> {
        Ok(self.status(session_id)? == KernelStatus::Ready)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_persist_between_evaluations() {
        let kernel = RhaiKernel::new();
        let req1 = ExecRequest::new("test-ses", "let x = 40 + 2;").unwrap();
        let res1 = kernel.execute(&req1).unwrap();
        assert!(res1.ok);

        let req2 = ExecRequest::new("test-ses", "x + 10").unwrap();
        let res2 = kernel.execute(&req2).unwrap();
        assert!(res2.ok);
        assert_eq!(res2.result.as_deref(), Some("52"));
    }

    #[test]
    fn stdout_print_and_return_values_are_separated() {
        let kernel = RhaiKernel::new();
        let req = ExecRequest::new("test-ses", "print(\"hello from rhai\"); 123").unwrap();
        let outcome = kernel.execute(&req).unwrap();
        assert!(outcome.ok);
        assert_eq!(outcome.stdout, "hello from rhai\n");
        assert_eq!(outcome.result.as_deref(), Some("123"));
    }

    #[test]
    fn vars_inspection_lists_all_bindings() {
        let kernel = RhaiKernel::new();
        let req = ExecRequest::new("test-ses", "let msg = \"hello\"; const PI = 3.14;").unwrap();
        kernel.execute(&req).unwrap();

        let vars = kernel.vars("test-ses").unwrap();
        assert_eq!(vars.len(), 2);
        assert_eq!(vars[0].name, "PI");
        assert_eq!(vars[1].name, "msg");
    }

    #[test]
    fn execution_failure_does_not_corrupt_existing_scope() {
        let kernel = RhaiKernel::new();
        let req1 = ExecRequest::new("test-ses", "let a = 100;").unwrap();
        kernel.execute(&req1).unwrap();

        // Run failing code
        let req_bad = ExecRequest::new("test-ses", "a = a + non_existent_var;").unwrap();
        let res_bad = kernel.execute(&req_bad).unwrap();
        assert!(!res_bad.ok);
        assert!(res_bad.error.is_some());

        // Previous variable should remain intact
        let req2 = ExecRequest::new("test-ses", "a").unwrap();
        let res2 = kernel.execute(&req2).unwrap();
        assert!(res2.ok);
        assert_eq!(res2.result.as_deref(), Some("100"));
    }

    #[test]
    fn reset_clears_scope() {
        let kernel = RhaiKernel::new();
        let req1 = ExecRequest::new("test-ses", "let a = 1;").unwrap();
        kernel.execute(&req1).unwrap();

        kernel.reset("test-ses").unwrap();
        let vars = kernel.vars("test-ses").unwrap();
        assert!(vars.is_empty());
    }

    #[test]
    fn runaway_loop_exceeds_max_operations_and_reports_timeout() {
        let kernel = RhaiKernel::new().with_operations_limit(Some(100));
        let req = ExecRequest::new("test-ses", "while true { }").unwrap();
        let outcome = kernel.execute(&req).unwrap();
        assert!(!outcome.ok);
        assert!(outcome.timed_out);
    }
}
