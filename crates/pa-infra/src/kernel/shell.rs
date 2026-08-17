//! A shell "namespace": exported variables and a working directory.
//!
//! This kernel needs no daemon, because there is no heap of live objects to
//! keep alive — the domain says so out loud in
//! [`KernelLanguage::holds_objects`]. What persists between calls is what a
//! shell can actually persist: `cd` and `export`. Pretending otherwise would be
//! the dishonest option; refusing to offer a shell kernel at all would be the
//! useless one.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use pa_domain::error::{DomainError, Result};
use pa_domain::kernel::{ExecOutcome, ExecRequest, KernelLanguage, KernelStatus, VarSummary};
use pa_domain::ports::KernelPort;
use serde::{Deserialize, Serialize};

use crate::files::{read_json, write_json};
use crate::paths::Paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ShellState {
    cwd: Option<String>,
    #[serde(default)]
    exports: BTreeMap<String, String>,
}

pub struct ShellKernel {
    paths: Paths,
    shell: String,
    default_cwd: String,
}

impl std::fmt::Debug for ShellKernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShellKernel")
    }
}

impl ShellKernel {
    pub fn new(paths: Paths, default_cwd: String) -> Self {
        Self {
            paths,
            shell: std::env::var("PA_KERNEL_SHELL").unwrap_or_else(|_| "bash".to_string()),
            default_cwd,
        }
    }

    fn state_path(&self, session_id: &str) -> PathBuf {
        self.paths.artifacts(session_id).join("shell-state.json")
    }

    fn state(&self, session_id: &str) -> Result<ShellState> {
        read_json(&self.state_path(session_id))
    }
}

impl KernelPort for ShellKernel {
    fn language(&self) -> KernelLanguage {
        KernelLanguage::Shell
    }

    fn status(&self, session_id: &str) -> Result<KernelStatus> {
        Ok(if self.state_path(session_id).exists() {
            KernelStatus::Ready
        } else {
            KernelStatus::Cold
        })
    }

    fn ensure(&self, session_id: &str) -> Result<KernelStatus> {
        if !self.state_path(session_id).exists() {
            write_json(&self.state_path(session_id), &ShellState::default())?;
        }
        Ok(KernelStatus::Ready)
    }

    fn execute(&self, request: &ExecRequest) -> Result<ExecOutcome> {
        self.ensure(&request.session_id)?;
        let mut state = self.state(&request.session_id)?;
        let cwd = state.cwd.clone().unwrap_or_else(|| self.default_cwd.clone());

        let dir = tempdir_for(&request.session_id)?;
        let pwd_file = dir.join("pwd");
        let env_file = dir.join("env");

        // The trailing capture runs whatever the code left behind: the shell's
        // final directory and exported environment become the next call's
        // starting state.
        let script = format!(
            "cd {cwd} 2>/dev/null || cd {home}\n\
             {code}\n\
             __pa_status=$?\n\
             pwd > {pwd_file}\n\
             env -0 > {env_file}\n\
             exit $__pa_status",
            cwd = shell_quote(&cwd),
            home = shell_quote(&self.default_cwd),
            code = request.code,
            pwd_file = shell_quote(&pwd_file.to_string_lossy()),
            env_file = shell_quote(&env_file.to_string_lossy()),
        );

        let started = Instant::now();
        let mut command = Command::new(&self.shell);
        command.arg("-c").arg(&script);
        for (key, value) in &state.exports {
            command.env(key, value);
        }
        // `timeout` bounds the run when it is available; without it a hung
        // command would hold the CLI open, which is worse than the dependency.
        let output = if let Some(timeout) = crate::kernel::socket::which("timeout") {
            let mut wrapped = Command::new(timeout);
            wrapped
                .arg(request.timeout_secs.to_string())
                .arg(&self.shell)
                .arg("-c")
                .arg(&script);
            for (key, value) in &state.exports {
                wrapped.env(key, value);
            }
            wrapped.output()
        } else {
            command.output()
        }
        .map_err(|error| DomainError::adapter(format!("run {}", self.shell), error))?;

        let code = output.status.code();
        // 124 is `timeout`'s signal that it killed the command.
        let timed_out = code == Some(124);

        if let Ok(text) = std::fs::read_to_string(&pwd_file) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                state.cwd = Some(trimmed.to_string());
            }
        }
        if let Ok(bytes) = std::fs::read(&env_file) {
            state.exports = parse_env0(&bytes);
        }
        write_json(&self.state_path(&request.session_id), &state)?;
        let _ = std::fs::remove_dir_all(&dir);

        Ok(ExecOutcome {
            ok: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            result: None,
            error: if output.status.success() {
                None
            } else if timed_out {
                Some(format!("timed out after {}s", request.timeout_secs))
            } else {
                Some(format!("exit {}", code.unwrap_or(-1)))
            },
            duration_ms: started.elapsed().as_millis() as u64,
            truncated_bytes: 0,
            timed_out,
        })
    }

    fn vars(&self, session_id: &str) -> Result<Vec<VarSummary>> {
        let state = self.state(session_id)?;
        let inherited: BTreeMap<String, String> = std::env::vars().collect();

        let mut vars: Vec<VarSummary> = state
            .exports
            .iter()
            // Only what this session actually changed. Listing the ambient
            // environment would bury the two variables that matter.
            .filter(|(key, value)| inherited.get(*key) != Some(value))
            // The shell maintains these itself; reporting them as things the
            // user set is noise, and PWD is already shown as the directory.
            .filter(|(key, _)| !matches!(key.as_str(), "PWD" | "OLDPWD" | "SHLVL" | "_"))
            .map(|(key, value)| VarSummary {
                name: key.clone(),
                type_name: "export".into(),
                length: Some(value.len() as u64),
                size_bytes: Some(value.len() as u64),
                preview: Some(clip(value, 96)),
            })
            .collect();

        if let Some(cwd) = state.cwd {
            vars.insert(
                0,
                VarSummary {
                    name: "PWD".into(),
                    type_name: "cwd".into(),
                    length: None,
                    size_bytes: None,
                    preview: Some(cwd),
                },
            );
        }
        Ok(vars)
    }

    fn reset(&self, session_id: &str) -> Result<()> {
        write_json(&self.state_path(session_id), &ShellState::default())
    }

    fn shutdown(&self, session_id: &str) -> Result<()> {
        let _ = std::fs::remove_file(self.state_path(session_id));
        Ok(())
    }

    fn snapshot(&self, session_id: &str) -> Result<Option<String>> {
        // The state file *is* the snapshot; there is nothing else to write.
        Ok(self
            .state_path(session_id)
            .exists()
            .then(|| self.state_path(session_id).to_string_lossy().to_string()))
    }

    fn restore(&self, session_id: &str) -> Result<bool> {
        Ok(self.state_path(session_id).exists())
    }
}

fn tempdir_for(session_id: &str) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!(
        "pa-shell-{}-{}-{}",
        session_id,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir)
        .map_err(|error| DomainError::adapter("create shell scratch", error))?;
    Ok(dir)
}

/// Wraps a value in single quotes, escaping any it contains.
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn parse_env0(bytes: &[u8]) -> BTreeMap<String, String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|chunk| !chunk.is_empty())
        .filter_map(|chunk| {
            let text = String::from_utf8_lossy(chunk);
            text.split_once('=')
                .map(|(key, value)| (key.to_string(), value.to_string()))
        })
        .collect()
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max - 3).chain("...".chars()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel(name: &str) -> ShellKernel {
        let dir = std::env::temp_dir().join(format!("pa-shell-k-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        ShellKernel::new(Paths::at(dir).unwrap(), "/tmp".into())
    }

    fn available() -> bool {
        crate::kernel::socket::which("bash").is_some()
    }

    #[test]
    fn shell_quoting_survives_an_apostrophe() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn env0_parses_into_pairs() {
        let parsed = parse_env0(b"A=1\0B=two\0MALFORMED\0");
        assert_eq!(parsed.get("A"), Some(&"1".to_string()));
        assert_eq!(parsed.get("B"), Some(&"two".to_string()));
        assert_eq!(parsed.len(), 2);
    }

    #[test]
    fn a_directory_change_persists_to_the_next_call() {
        if !available() {
            return;
        }
        let kernel = kernel("cd");
        kernel
            .execute(&ExecRequest::new("ses-1", "cd /usr").unwrap())
            .unwrap();
        let outcome = kernel
            .execute(&ExecRequest::new("ses-1", "pwd").unwrap())
            .unwrap();
        assert_eq!(outcome.stdout.trim(), "/usr");
    }

    #[test]
    fn an_export_persists_and_shows_up_in_vars() {
        if !available() {
            return;
        }
        let kernel = kernel("export");
        kernel
            .execute(&ExecRequest::new("ses-1", "export TOKEN=abc123").unwrap())
            .unwrap();

        let outcome = kernel
            .execute(&ExecRequest::new("ses-1", "echo $TOKEN").unwrap())
            .unwrap();
        assert_eq!(outcome.stdout.trim(), "abc123");

        let vars = kernel.vars("ses-1").unwrap();
        assert!(vars.iter().any(|var| var.name == "TOKEN"));
        // The directory is reported once, as a directory — not also as an
        // export alongside the shell's own bookkeeping.
        assert_eq!(vars.iter().filter(|var| var.name == "PWD").count(), 1);
        assert_eq!(vars[0].type_name, "cwd");
        assert!(vars.iter().all(|var| var.name != "SHLVL"));
    }

    #[test]
    fn a_failing_command_reports_its_exit_code() {
        if !available() {
            return;
        }
        let kernel = kernel("fail");
        let outcome = kernel
            .execute(&ExecRequest::new("ses-1", "exit 3").unwrap())
            .unwrap();
        assert!(!outcome.ok);
        assert_eq!(outcome.error.as_deref(), Some("exit 3"));
    }

    #[test]
    fn reset_forgets_the_directory_and_the_exports() {
        if !available() {
            return;
        }
        let kernel = kernel("reset");
        kernel
            .execute(&ExecRequest::new("ses-1", "cd /usr && export GONE=1").unwrap())
            .unwrap();
        kernel.reset("ses-1").unwrap();
        assert!(kernel
            .vars("ses-1")
            .unwrap()
            .iter()
            .all(|var| var.name != "GONE"));
    }
}
