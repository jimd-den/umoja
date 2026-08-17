//! A kernel held in a separate long-lived process, reached over a Unix socket.
//!
//! `pa` is a one-shot binary — it starts, does one thing and exits — so the
//! namespace cannot live inside it. It lives in a daemon this module starts on
//! first use and leaves running. That is what makes
//!
//! ```text
//! pa kernel exec 'rows = load("huge.json")'
//! pa kernel exec 'len([r for r in rows if r["status"] == 500])'
//! ```
//!
//! two commands against one dataset instead of two loads.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use pa_domain::error::{DomainError, Result};
use pa_domain::kernel::{ExecOutcome, ExecRequest, KernelLanguage, KernelStatus, VarSummary};
use pa_domain::ports::KernelPort;
use serde::Deserialize;
use serde_json::json;

use crate::hash::digest;
use crate::paths::{ensure_parent, Paths};

const PYTHON_BOOTSTRAP: &str = include_str!("bootstrap.py");
const NODE_BOOTSTRAP: &str = include_str!("bootstrap.js");

/// How long a kernel sits unused before it exits and gives back its memory.
const IDLE_SECONDS: u64 = 3_600;
const START_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
pub struct KernelConfig {
    pub language: KernelLanguage,
    /// The interpreter to run. `$PA_KERNEL_PYTHON` / `$PA_KERNEL_NODE` win when
    /// set, so a project can point at its own virtualenv.
    pub interpreter: String,
    pub idle_seconds: u64,
}

impl KernelConfig {
    pub fn for_language(language: KernelLanguage) -> Self {
        let interpreter = match language {
            KernelLanguage::Python => std::env::var("PA_KERNEL_PYTHON")
                .unwrap_or_else(|_| "python3".to_string()),
            KernelLanguage::Node => {
                std::env::var("PA_KERNEL_NODE").unwrap_or_else(|_| "node".to_string())
            }
            KernelLanguage::Shell => "sh".to_string(),
        };
        Self {
            language,
            interpreter,
            idle_seconds: IDLE_SECONDS,
        }
    }
}

pub struct SocketKernel {
    paths: Paths,
    config: KernelConfig,
}

impl std::fmt::Debug for SocketKernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SocketKernel({})", self.config.language.label())
    }
}

#[derive(Debug, Deserialize)]
struct RawExec {
    #[serde(default)]
    ok: bool,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    #[serde(default)]
    result: Option<String>,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    duration_ms: u64,
    #[serde(default)]
    timed_out: bool,
}

#[derive(Debug, Deserialize)]
struct RawVars {
    #[serde(default)]
    vars: Vec<VarSummary>,
}

impl SocketKernel {
    pub fn new(paths: Paths, config: KernelConfig) -> Self {
        Self { paths, config }
    }

    fn bootstrap_source(&self) -> Result<(&'static str, &'static str)> {
        match self.config.language {
            KernelLanguage::Python => Ok(("pa_kernel_bootstrap.py", PYTHON_BOOTSTRAP)),
            KernelLanguage::Node => Ok(("pa_kernel_bootstrap.js", NODE_BOOTSTRAP)),
            KernelLanguage::Shell => Err(DomainError::Unsupported(
                "the shell kernel does not use a socket; use ShellKernel".into(),
            )),
        }
    }

    /// Writes the bootstrap script out, refreshing it when this binary is newer.
    ///
    /// The script is embedded in the binary, so an upgraded `pa` must not keep
    /// talking to a stale kernel script left by the previous version.
    fn install_bootstrap(&self) -> Result<PathBuf> {
        let (name, source) = self.bootstrap_source()?;
        let path = self.paths.runtime(name);
        ensure_parent(&path)?;

        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current != source {
            std::fs::write(&path, source).map_err(|error| {
                DomainError::adapter(format!("write {}", path.display()), error)
            })?;
        }
        Ok(path)
    }

    /// Sockets live in the runtime directory under a short hashed name.
    ///
    /// `AF_UNIX` paths are capped at about 108 bytes by the kernel, and the
    /// natural home — `<prime-home>/session-artifacts/<session-id>/` — blows
    /// through that on any reasonably nested directory. Hashing the identity
    /// into a fixed 16 characters makes the length constant regardless of how
    /// deep the workspace is.
    fn socket_path(&self, session_id: &str) -> PathBuf {
        let identity = format!(
            "{}|{session_id}|{}",
            self.paths.root().display(),
            self.config.language.label()
        );
        socket_dir().join(format!("pa-{}.sock", digest(&identity)))
    }

    fn snapshot_path(&self, session_id: &str) -> PathBuf {
        let base = self.paths.kernel_state(session_id);
        match self.config.language {
            KernelLanguage::Python => base.with_extension("pkl"),
            _ => base,
        }
    }

    /// One request, one response. The connection is not held between calls —
    /// the *namespace* is what persists, not the socket.
    fn request(&self, session_id: &str, payload: serde_json::Value) -> Result<serde_json::Value> {
        let path = self.socket_path(session_id);
        let mut stream = UnixStream::connect(&path).map_err(|error| {
            DomainError::adapter(format!("connect {}", path.display()), error)
        })?;

        let timeout = payload
            .get("timeout")
            .and_then(|value| value.as_u64())
            .unwrap_or(120)
            .saturating_add(15);
        stream
            .set_read_timeout(Some(Duration::from_secs(timeout)))
            .map_err(|error| DomainError::adapter("kernel timeout", error))?;

        let mut line = serde_json::to_string(&payload)
            .map_err(|error| DomainError::adapter("encode request", error))?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|error| DomainError::adapter("write to kernel", error))?;
        stream
            .flush()
            .map_err(|error| DomainError::adapter("flush to kernel", error))?;

        let mut response = String::new();
        BufReader::new(&stream)
            .read_line(&mut response)
            .map_err(|error| DomainError::adapter("read from kernel", error))?;

        if response.trim().is_empty() {
            return Err(DomainError::adapter(
                "kernel",
                "the kernel closed the connection without answering",
            ));
        }

        serde_json::from_str(&response)
            .map_err(|error| DomainError::parse("kernel response", error))
    }

    fn is_listening(&self, session_id: &str) -> bool {
        UnixStream::connect(self.socket_path(session_id)).is_ok()
    }

    fn spawn(&self, session_id: &str) -> Result<()> {
        let script = self.install_bootstrap()?;
        let socket = self.socket_path(session_id);
        ensure_parent(&socket)?;
        // The socket no longer lives under the artifact directory, so that
        // directory has to be created deliberately — the kernel log goes there.
        std::fs::create_dir_all(self.paths.artifacts(session_id))
            .map_err(|error| DomainError::adapter("create session artifacts", error))?;

        // A socket file left by a kernel that died would make connect() fail
        // with ECONNREFUSED forever.
        if socket.exists() && !self.is_listening(session_id) {
            let _ = std::fs::remove_file(&socket);
        }

        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.paths.kernel_log(session_id))
            .map_err(|error| DomainError::adapter("open kernel log", error))?;

        let mut command = Self::detached(&self.config.interpreter);
        command
            .arg(&script)
            .arg("--socket")
            .arg(&socket)
            .arg("--idle")
            .arg(self.config.idle_seconds.to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::from(
                log.try_clone()
                    .map_err(|error| DomainError::adapter("clone log handle", error))?,
            ))
            .stderr(Stdio::from(log));

        command.spawn().map_err(|error| {
            DomainError::adapter(
                format!("start {} kernel", self.config.language.label()),
                format!("{error}: is '{}' installed?", self.config.interpreter),
            )
        })?;

        // Wait for the socket to answer rather than assuming it will.
        let deadline = Instant::now() + START_TIMEOUT;
        while Instant::now() < deadline {
            if self.is_listening(session_id) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        Err(DomainError::adapter(
            format!("start {} kernel", self.config.language.label()),
            format!(
                "no response within {}s; see {}",
                START_TIMEOUT.as_secs(),
                self.paths.kernel_log(session_id).display()
            ),
        ))
    }

    /// Runs the interpreter under `setsid` when available so the kernel
    /// survives the terminal that started it — closing the shell must not take
    /// the namespace with it.
    fn detached(interpreter: &str) -> Command {
        if which("setsid").is_some() {
            let mut command = Command::new("setsid");
            command.arg(interpreter);
            command
        } else {
            Command::new(interpreter)
        }
    }
}

impl KernelPort for SocketKernel {
    fn language(&self) -> KernelLanguage {
        self.config.language
    }

    fn status(&self, session_id: &str) -> Result<KernelStatus> {
        if !self.socket_path(session_id).exists() {
            return Ok(KernelStatus::Cold);
        }
        match self.request(session_id, json!({"op": "ping"})) {
            Ok(_) => Ok(KernelStatus::Ready),
            Err(_) => Ok(KernelStatus::Dead),
        }
    }

    fn ensure(&self, session_id: &str) -> Result<KernelStatus> {
        if self.is_listening(session_id) {
            return Ok(KernelStatus::Ready);
        }
        self.spawn(session_id)?;
        Ok(KernelStatus::Ready)
    }

    fn execute(&self, request: &ExecRequest) -> Result<ExecOutcome> {
        self.ensure(&request.session_id)?;

        let raw = self.request(
            &request.session_id,
            json!({
                "op": "exec",
                "code": request.code,
                "timeout": request.timeout_secs,
            }),
        )?;

        let parsed: RawExec = serde_json::from_value(raw)
            .map_err(|error| DomainError::parse("kernel exec response", error))?;

        Ok(ExecOutcome {
            ok: parsed.ok,
            stdout: parsed.stdout,
            stderr: parsed.stderr,
            result: parsed.result,
            error: parsed.error,
            duration_ms: parsed.duration_ms,
            truncated_bytes: 0,
            timed_out: parsed.timed_out,
        })
    }

    fn vars(&self, session_id: &str) -> Result<Vec<VarSummary>> {
        if !self.is_listening(session_id) {
            // A cold kernel has no variables; that is an answer, not a failure.
            return Ok(Vec::new());
        }
        let raw = self.request(session_id, json!({"op": "vars"}))?;
        let parsed: RawVars = serde_json::from_value(raw)
            .map_err(|error| DomainError::parse("kernel vars response", error))?;
        Ok(parsed.vars)
    }

    fn reset(&self, session_id: &str) -> Result<()> {
        if !self.is_listening(session_id) {
            return Ok(());
        }
        self.request(session_id, json!({"op": "reset"}))?;
        Ok(())
    }

    fn shutdown(&self, session_id: &str) -> Result<()> {
        if !self.is_listening(session_id) {
            return Ok(());
        }
        self.request(session_id, json!({"op": "shutdown"}))?;
        let _ = std::fs::remove_file(self.socket_path(session_id));
        Ok(())
    }

    fn snapshot(&self, session_id: &str) -> Result<Option<String>> {
        if !self.is_listening(session_id) {
            return Ok(None);
        }
        let path = self.snapshot_path(session_id);
        ensure_parent(&path)?;
        let raw = self.request(
            session_id,
            json!({"op": "snapshot", "path": path.to_string_lossy()}),
        )?;
        if raw.get("ok").and_then(|value| value.as_bool()) == Some(true) {
            Ok(Some(path.to_string_lossy().to_string()))
        } else {
            Ok(None)
        }
    }

    fn restore(&self, session_id: &str) -> Result<bool> {
        let path = self.snapshot_path(session_id);
        if !path.exists() {
            return Ok(false);
        }
        self.ensure(session_id)?;
        let raw = self.request(
            session_id,
            json!({"op": "restore", "path": path.to_string_lossy()}),
        )?;
        Ok(raw.get("ok").and_then(|value| value.as_bool()) == Some(true))
    }
}

/// Where short-named sockets go: the per-user runtime directory when there is
/// one, otherwise the temporary directory.
fn socket_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|dir| dir.is_dir())
        .unwrap_or_else(std::env::temp_dir)
}

/// Finds an executable on `PATH`.
pub fn which(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("pa-kernel-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Paths::at(dir).unwrap()
    }

    fn python(name: &str) -> SocketKernel {
        SocketKernel::new(
            paths(name),
            KernelConfig::for_language(KernelLanguage::Python),
        )
    }

    #[test]
    fn which_finds_a_real_binary_and_not_a_fictional_one() {
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-real-binary-xyzzy").is_none());
    }

    #[test]
    fn socket_paths_stay_within_the_af_unix_limit() {
        // The bug this guards against showed up only at runtime: a deeply
        // nested home made bind() fail with "AF_UNIX path too long".
        let deep = std::env::temp_dir()
            .join(format!("pa-deep-{}", std::process::id()))
            .join("a".repeat(90))
            .join("b".repeat(90));
        let kernel = SocketKernel::new(
            Paths::at(deep).unwrap(),
            KernelConfig::for_language(KernelLanguage::Python),
        );

        let path = kernel.socket_path("ses-with-a-very-long-identifier-indeed-0000-abcd");
        assert!(
            path.to_string_lossy().len() < 100,
            "socket path is {} bytes: {}",
            path.to_string_lossy().len(),
            path.display()
        );
    }

    #[test]
    fn a_cold_kernel_reports_cold_and_has_no_vars() {
        let kernel = python("cold");
        assert_eq!(kernel.status("ses-1").unwrap(), KernelStatus::Cold);
        assert!(kernel.vars("ses-1").unwrap().is_empty());
        // Shutting down something that was never started is not an error.
        assert!(kernel.shutdown("ses-1").is_ok());
    }

    #[test]
    fn the_bootstrap_is_written_out_and_refreshed() {
        let kernel = python("bootstrap");
        let first = kernel.install_bootstrap().unwrap();
        assert!(first.exists());

        std::fs::write(&first, "stale").unwrap();
        let second = kernel.install_bootstrap().unwrap();
        assert_eq!(first, second);
        assert!(std::fs::read_to_string(&second).unwrap().contains("NAMESPACE"));
    }

    // The tests below need a real interpreter. They are the ones that prove the
    // feature actually works, so they run whenever python3 is present.
    fn python_available() -> bool {
        which("python3").is_some()
    }

    #[test]
    fn the_namespace_outlives_the_process_that_wrote_it() {
        if !python_available() {
            return;
        }
        let paths = paths("persist");
        let session = "ses-persist";

        {
            let kernel = SocketKernel::new(
                paths.clone(),
                KernelConfig::for_language(KernelLanguage::Python),
            );
            let outcome = kernel
                .execute(&ExecRequest::new(session, "rows = list(range(100000))").unwrap())
                .unwrap();
            assert!(outcome.ok, "{outcome:?}");
        }

        // A brand-new client object, exactly as a second `pa` invocation would
        // build: the data is still there.
        let reconnected = SocketKernel::new(
            paths.clone(),
            KernelConfig::for_language(KernelLanguage::Python),
        );
        let outcome = reconnected
            .execute(&ExecRequest::new(session, "len(rows)").unwrap())
            .unwrap();
        assert_eq!(outcome.result.as_deref(), Some("100000"));

        let vars = reconnected.vars(session).unwrap();
        assert!(vars.iter().any(|var| var.name == "rows" && var.length == Some(100_000)));

        reconnected.shutdown(session).unwrap();
    }

    #[test]
    fn errors_come_back_as_outcomes_and_leave_the_kernel_alive() {
        if !python_available() {
            return;
        }
        let kernel = python("errors");
        let session = "ses-errors";

        kernel
            .execute(&ExecRequest::new(session, "keeper = 42").unwrap())
            .unwrap();
        let broken = kernel
            .execute(&ExecRequest::new(session, "1/0").unwrap())
            .unwrap();
        assert!(!broken.ok);
        assert!(broken.error.unwrap().contains("ZeroDivisionError"));

        // The namespace survived the exception.
        let after = kernel
            .execute(&ExecRequest::new(session, "keeper").unwrap())
            .unwrap();
        assert_eq!(after.result.as_deref(), Some("42"));

        kernel.shutdown(session).unwrap();
    }

    #[test]
    fn a_runaway_loop_times_out_without_killing_the_namespace() {
        if !python_available() {
            return;
        }
        let kernel = python("timeout");
        let session = "ses-timeout";

        kernel
            .execute(&ExecRequest::new(session, "keeper = 'alive'").unwrap())
            .unwrap();
        let outcome = kernel
            .execute(
                &ExecRequest::new(session, "while True:\n    pass")
                    .unwrap()
                    .with_timeout(1),
            )
            .unwrap();
        assert!(outcome.timed_out);

        let after = kernel
            .execute(&ExecRequest::new(session, "keeper").unwrap())
            .unwrap();
        assert_eq!(after.result.as_deref(), Some("'alive'"));

        kernel.shutdown(session).unwrap();
    }

    #[test]
    fn a_snapshot_restores_into_a_fresh_kernel() {
        if !python_available() {
            return;
        }
        let kernel = python("snapshot");
        let session = "ses-snapshot";

        kernel
            .execute(&ExecRequest::new(session, "answer = 42").unwrap())
            .unwrap();
        assert!(kernel.snapshot(session).unwrap().is_some());
        kernel.shutdown(session).unwrap();

        assert!(kernel.restore(session).unwrap());
        let outcome = kernel
            .execute(&ExecRequest::new(session, "answer").unwrap())
            .unwrap();
        assert_eq!(outcome.result.as_deref(), Some("42"));

        kernel.shutdown(session).unwrap();
    }

    #[test]
    fn reset_clears_bindings_but_keeps_the_process() {
        if !python_available() {
            return;
        }
        let kernel = python("reset");
        let session = "ses-reset";

        kernel
            .execute(&ExecRequest::new(session, "gone = 1").unwrap())
            .unwrap();
        kernel.reset(session).unwrap();
        assert!(kernel.vars(session).unwrap().is_empty());
        assert_eq!(kernel.status(session).unwrap(), KernelStatus::Ready);

        kernel.shutdown(session).unwrap();
    }

    #[test]
    fn stdout_and_the_final_expression_are_reported_separately() {
        if !python_available() {
            return;
        }
        let kernel = python("stdout");
        let session = "ses-stdout";

        let outcome = kernel
            .execute(
                &ExecRequest::new(session, "print('hello')\n2 + 2").unwrap(),
            )
            .unwrap();
        assert_eq!(outcome.stdout.trim(), "hello");
        assert_eq!(outcome.result.as_deref(), Some("4"));

        kernel.shutdown(session).unwrap();
    }
}
