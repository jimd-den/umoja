//! Agent runners: the adapters that actually make a model do something.
//!
//! Two harnesses ship here, Claude Code and opencode, behind one
//! [`AgentRunner`] trait. Adding a third is one struct and one entry in
//! [`build`] — nothing above this file changes, because nothing above this file
//! knows which harness is in use.

use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Instant;

use std::collections::HashMap;
use std::sync::Mutex;

use umoja_domain::error::{DomainError, Result};
use umoja_domain::ports::{AgentRunner, RunnerRegistry};
use umoja_domain::runner::{RunOutcome, RunRequest, RunnerCapabilities};
use umoja_domain::session::Usage;

use crate::kernel::socket::which;

/// Everything the CLI runners share.
fn spawn_or_wait(
    mut command: Command,
    request: &RunRequest,
    started: Instant,
    parse: impl Fn(&str, &str) -> (String, Usage, Option<String>),
) -> Result<RunOutcome> {
    command.current_dir(&request.workdir);

    if request.detached {
        // A detached run is a delegation, not a call. The pid comes back so the
        // registry can track it; the answer arrives later through a message or
        // a file, exactly as `rlm(...)` specifies.
        let child = command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| DomainError::adapter("spawn agent", error))?;
        return Ok(RunOutcome::detached(child.id()));
    }

    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|error| DomainError::adapter("run agent", error))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let (text, usage, session) = parse(&stdout, &stderr);

    Ok(RunOutcome {
        ok: output.status.success(),
        text,
        usage,
        runner_session: session,
        exit_code: output.status.code(),
        error: if output.status.success() {
            None
        } else {
            // The harness's own error text, verbatim. A friendlier paraphrase
            // would only hide which of the two tools actually failed.
            Some(if stderr.trim().is_empty() {
                format!("exit {}", output.status.code().unwrap_or(-1))
            } else {
                stderr.trim().to_string()
            })
        },
        pid: None,
        duration_ms: started.elapsed().as_millis() as u64,
    })
}

fn probe(binary: &str, name: &str) -> Result<()> {
    which(binary).map(|_| ()).ok_or_else(|| {
        DomainError::Unsupported(format!(
            "'{binary}' is not on PATH, so the {name} runner cannot start anything"
        ))
    })
}

/// Claude Code, driven headlessly with `claude -p --output-format json`.
#[derive(Debug, Default)]
pub struct ClaudeRunner {
    binary: String,
}

impl ClaudeRunner {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("PA_CLAUDE_BIN").unwrap_or_else(|_| "claude".to_string()),
        }
    }
}

impl AgentRunner for ClaudeRunner {
    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            name: "claude".into(),
            can_resume: true,
            can_stream: true,
            reports_usage: true,
            supports_system_prompt: true,
            supports_model_selection: true,
        }
    }

    fn run(&self, request: &RunRequest) -> Result<RunOutcome> {
        let started = Instant::now();
        let mut command = Command::new(&self.binary);
        command.arg("-p").arg(&request.prompt);
        command.args(["--output-format", "json"]);

        if let Some(model) = &request.model {
            command.args(["--model", model]);
        }
        if let Some(system) = &request.system_prompt {
            command.args(["--append-system-prompt", system]);
        }
        if let Some(session) = &request.runner_session {
            command.args(["--resume", session]);
        }

        spawn_or_wait(command, request, started, parse_claude_json)
    }

    fn probe(&self) -> Result<()> {
        probe(&self.binary, "claude")
    }
}

/// Claude's `--output-format json` result envelope.
fn parse_claude_json(stdout: &str, stderr: &str) -> (String, Usage, Option<String>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(stdout.trim()) else {
        // Not JSON: the run probably failed before Claude produced a result.
        // Returning the raw text beats returning nothing.
        let text = if stdout.trim().is_empty() {
            stderr.trim().to_string()
        } else {
            stdout.trim().to_string()
        };
        return (text, Usage::default(), None);
    };

    let text = value
        .get("result")
        .and_then(|result| result.as_str())
        .unwrap_or_default()
        .to_string();

    let usage = value
        .get("usage")
        .map(|usage| Usage {
            input_tokens: field(usage, "input_tokens"),
            output_tokens: field(usage, "output_tokens"),
            turns: 1,
            attributed_child_tokens: 0,
        })
        .unwrap_or(Usage {
            turns: 1,
            ..Default::default()
        });

    let session = value
        .get("session_id")
        .and_then(|id| id.as_str())
        .map(str::to_string);

    (text, usage, session)
}

fn field(value: &serde_json::Value, name: &str) -> u64 {
    value.get(name).and_then(|v| v.as_u64()).unwrap_or(0)
}

/// opencode, driven with `opencode run --format json`.
#[derive(Debug, Default)]
pub struct OpencodeRunner {
    binary: String,
}

impl OpencodeRunner {
    pub fn new() -> Self {
        Self {
            binary: std::env::var("PA_OPENCODE_BIN").unwrap_or_else(|_| "opencode".to_string()),
        }
    }
}

impl AgentRunner for OpencodeRunner {
    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            name: "opencode".into(),
            can_resume: true,
            can_stream: true,
            // opencode's event stream does not report a usage envelope the way
            // Claude's result does, so budgets fall back to counting turns.
            reports_usage: false,
            supports_system_prompt: false,
            supports_model_selection: true,
        }
    }

    fn run(&self, request: &RunRequest) -> Result<RunOutcome> {
        let started = Instant::now();
        let mut command = Command::new(&self.binary);
        command.arg("run").args(["--format", "json"]);

        if let Some(model) = &request.model {
            command.args(["--model", model]);
        }
        if let Some(session) = &request.runner_session {
            command.args(["--session", session]);
        }
        command.args(["--dir", &request.workdir]);

        // opencode has no system-prompt flag, so the instruction is prepended
        // to the message instead of being silently dropped.
        let message = match &request.system_prompt {
            Some(system) => format!("{system}\n\n---\n\n{}", request.prompt),
            None => request.prompt.clone(),
        };
        command.arg(&message);

        spawn_or_wait(command, request, started, parse_opencode_events)
    }

    fn probe(&self) -> Result<()> {
        probe(&self.binary, "opencode")
    }
}

/// opencode emits a stream of JSON events, one per line.
///
/// The text parts are concatenated in arrival order; anything unrecognised is
/// ignored rather than guessed at.
fn parse_opencode_events(stdout: &str, stderr: &str) -> (String, Usage, Option<String>) {
    let mut text = String::new();
    let mut session = None;
    let mut saw_json = false;

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        saw_json = true;

        if session.is_none() {
            session = event
                .get("sessionID")
                .or_else(|| event.get("session_id"))
                .or_else(|| event.get("properties").and_then(|p| p.get("sessionID")))
                .and_then(|id| id.as_str())
                .map(str::to_string);
        }

        collect_text(&event, &mut text);
    }

    if !saw_json {
        let raw = if stdout.trim().is_empty() {
            stderr.trim()
        } else {
            stdout.trim()
        };
        return (raw.to_string(), Usage { turns: 1, ..Default::default() }, None);
    }

    (
        text.trim().to_string(),
        Usage {
            turns: 1,
            ..Default::default()
        },
        session,
    )
}

/// Pulls assistant text out of an event without needing opencode's full schema.
fn collect_text(event: &serde_json::Value, out: &mut String) {
    // Only text parts belong in the answer. Tool calls and reasoning traces are
    // deliberately skipped: this is the reply, not the transcript.
    if event.get("type").and_then(|t| t.as_str()) == Some("text") {
        if let Some(text) = event.get("text").and_then(|t| t.as_str()) {
            out.push_str(text);
            return;
        }
    }

    for key in ["part", "properties", "data", "message"] {
        if let Some(nested) = event.get(key) {
            collect_text(nested, out);
        }
    }
}

/// Runs nothing, reports what it would have run.
///
/// This is what makes `pa tick --dry-run` safe to point at a live registry, and
/// what lets somebody try the whole tool before installing either harness.
#[derive(Debug, Default)]
pub struct DryRunner;

impl AgentRunner for DryRunner {
    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            name: "dry-run".into(),
            can_resume: false,
            can_stream: false,
            reports_usage: false,
            supports_system_prompt: true,
            supports_model_selection: true,
        }
    }

    fn run(&self, request: &RunRequest) -> Result<RunOutcome> {
        Ok(RunOutcome {
            ok: true,
            text: format!(
                "[dry run] would send to {}: {}",
                request.model.as_deref().unwrap_or("the default model"),
                request.prompt
            ),
            usage: Usage {
                turns: 1,
                ..Default::default()
            },
            runner_session: None,
            exit_code: Some(0),
            error: None,
            pid: None,
            duration_ms: 0,
        })
    }

    fn probe(&self) -> Result<()> {
        Ok(())
    }
}

/// Resolves runners by name, building each at most once.
///
/// A session that names a harness this machine does not have falls back to the
/// default rather than failing outright — a heartbeat should still fire when
/// the session was started on a laptop that had opencode and this one does not.
pub struct CachingRunnerRegistry {
    default_name: String,
    cache: Mutex<HashMap<String, Arc<dyn AgentRunner>>>,
}

impl std::fmt::Debug for CachingRunnerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CachingRunnerRegistry({})", self.default_name)
    }
}

impl CachingRunnerRegistry {
    pub fn new(default_name: impl Into<String>) -> Self {
        Self {
            default_name: default_name.into(),
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl RunnerRegistry for CachingRunnerRegistry {
    fn get(&self, name: &str) -> Result<Arc<dyn AgentRunner>> {
        let wanted = if name.trim().is_empty() {
            self.default_name.clone()
        } else {
            name.to_string()
        };

        if let Some(cached) = self.cache.lock().unwrap().get(&wanted) {
            return Ok(cached.clone());
        }

        let runner = match build(&wanted) {
            Ok(runner) if runner.probe().is_ok() => runner,
            // Either the name is unknown here or that harness is not installed.
            // Falling back keeps the session running; failing would strand it.
            _ => build(&self.default_name)?,
        };

        self.cache
            .lock()
            .unwrap()
            .insert(wanted, runner.clone());
        Ok(runner)
    }

    fn default_name(&self) -> String {
        self.default_name.clone()
    }
}

pub const RUNNERS: [&str; 3] = ["claude", "opencode", "dry-run"];

pub fn build(name: &str) -> Result<Arc<dyn AgentRunner>> {
    match name.trim().to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => Ok(Arc::new(ClaudeRunner::new())),
        "opencode" => Ok(Arc::new(OpencodeRunner::new())),
        "dry-run" | "dry" | "none" => Ok(Arc::new(DryRunner)),
        other => Err(DomainError::invalid(format!(
            "unknown runner '{other}'; expected one of {}",
            RUNNERS.join(", ")
        ))),
    }
}

/// Picks a runner that is actually installed, preferring `claude`.
pub fn detect() -> &'static str {
    if which(&std::env::var("PA_CLAUDE_BIN").unwrap_or_else(|_| "claude".into())).is_some() {
        return "claude";
    }
    if which(&std::env::var("PA_OPENCODE_BIN").unwrap_or_else(|_| "opencode".into())).is_some() {
        return "opencode";
    }
    "dry-run"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_json_yields_text_usage_and_session() {
        let stdout = r#"{"type":"result","result":"the answer","session_id":"abc-123",
            "usage":{"input_tokens":120,"output_tokens":30}}"#;
        let (text, usage, session) = parse_claude_json(stdout, "");
        assert_eq!(text, "the answer");
        assert_eq!(usage.input_tokens, 120);
        assert_eq!(usage.output_tokens, 30);
        assert_eq!(usage.turns, 1);
        assert_eq!(session.as_deref(), Some("abc-123"));
    }

    #[test]
    fn non_json_output_is_returned_rather_than_swallowed() {
        let (text, _, session) = parse_claude_json("", "command not found: claude");
        assert_eq!(text, "command not found: claude");
        assert!(session.is_none());
    }

    #[test]
    fn opencode_events_concatenate_their_text_parts() {
        let stdout = concat!(
            r#"{"type":"session.updated","properties":{"sessionID":"ses_9"}}"#,
            "\n",
            r#"{"part":{"type":"text","text":"first "}}"#,
            "\n",
            r#"{"part":{"type":"tool","tool":"bash"}}"#,
            "\n",
            r#"{"part":{"type":"text","text":"second"}}"#,
            "\n",
            "not json at all\n",
        );
        let (text, usage, session) = parse_opencode_events(stdout, "");
        assert_eq!(text, "first second");
        assert_eq!(session.as_deref(), Some("ses_9"));
        assert_eq!(usage.turns, 1);
    }

    #[test]
    fn opencode_plain_output_falls_back_to_the_raw_text() {
        let (text, _, _) = parse_opencode_events("just a plain answer", "");
        assert_eq!(text, "just a plain answer");
    }

    #[test]
    fn runners_are_built_by_name_and_unknown_ones_are_refused() {
        assert_eq!(build("claude").unwrap().capabilities().name, "claude");
        assert_eq!(build("OpenCode").unwrap().capabilities().name, "opencode");
        assert_eq!(build("dry-run").unwrap().capabilities().name, "dry-run");
        assert!(build("emacs").is_err());
    }

    #[test]
    fn the_dry_runner_reports_instead_of_running() {
        let request = RunRequest::new("ses-1", "do the thing", "/tmp")
            .unwrap()
            .with_model(Some("sonnet".into()));
        let outcome = DryRunner.run(&request).unwrap();
        assert!(outcome.ok);
        assert!(outcome.text.contains("would send to sonnet"));
        assert!(DryRunner.probe().is_ok());
    }

    #[test]
    fn the_registry_returns_the_named_runner_and_caches_it() {
        let registry = CachingRunnerRegistry::new("dry-run");
        assert_eq!(registry.get("dry-run").unwrap().capabilities().name, "dry-run");
        assert_eq!(registry.get("dry-run").unwrap().capabilities().name, "dry-run");
        assert_eq!(registry.cache.lock().unwrap().len(), 1);
    }

    #[test]
    fn an_unknown_or_uninstalled_harness_falls_back_to_the_default() {
        let registry = CachingRunnerRegistry::new("dry-run");
        // Neither of these can run here, so both land on the default rather
        // than stranding the session that named them.
        assert_eq!(registry.get("emacs").unwrap().capabilities().name, "dry-run");
        assert_eq!(registry.get("").unwrap().capabilities().name, "dry-run");
        assert_eq!(registry.default_name(), "dry-run");
    }

    #[test]
    fn a_missing_binary_is_reported_before_anything_is_attempted() {
        let runner = ClaudeRunner {
            binary: "definitely-not-installed-xyzzy".into(),
        };
        let error = runner.probe().unwrap_err();
        assert!(matches!(error, DomainError::Unsupported(_)));
        assert!(error.to_string().contains("not on PATH"));
    }
}
