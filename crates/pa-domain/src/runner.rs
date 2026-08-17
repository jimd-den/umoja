//! What it means to actually run an agent turn.
//!
//! This is the seam that makes the whole tool harness-agnostic. Claude Code and
//! opencode are two adapters behind one trait; nothing above this line knows
//! which is in use, and adding a third is one file.

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};
use crate::heartbeat::DeliveryMode;
use crate::session::Usage;

/// One turn to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRequest {
    pub session_id: String,
    /// The runner's own session handle, when it has one, so a follow-up turn
    /// continues the same conversation instead of starting a new one.
    pub runner_session: Option<String>,
    pub prompt: String,
    pub system_prompt: Option<String>,
    pub model: Option<String>,
    pub workdir: String,
    /// How this prompt relates to work already in flight.
    pub delivery: DeliveryMode,
    pub timeout_secs: u64,
    /// Run without waiting; the process is detached and reports later.
    pub detached: bool,
}

impl RunRequest {
    pub const DEFAULT_TIMEOUT_SECS: u64 = 900;

    pub fn new(
        session_id: impl Into<String>,
        prompt: impl Into<String>,
        workdir: impl Into<String>,
    ) -> Result<Self> {
        let prompt = prompt.into().trim().to_string();
        if prompt.is_empty() {
            return Err(DomainError::invalid("a run needs a prompt"));
        }
        Ok(Self {
            session_id: session_id.into(),
            runner_session: None,
            prompt,
            system_prompt: None,
            model: None,
            workdir: workdir.into(),
            delivery: DeliveryMode::Auto,
            timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
            detached: false,
        })
    }

    pub fn with_model(mut self, model: Option<String>) -> Self {
        self.model = model;
        self
    }

    pub fn with_system_prompt(mut self, prompt: Option<String>) -> Self {
        self.system_prompt = prompt;
        self
    }

    pub fn detached(mut self) -> Self {
        self.detached = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunOutcome {
    pub ok: bool,
    pub text: String,
    #[serde(default)]
    pub usage: Usage,
    pub runner_session: Option<String>,
    pub exit_code: Option<i32>,
    pub error: Option<String>,
    /// Set when the run was launched detached; there is no text yet.
    pub pid: Option<u32>,
    pub duration_ms: u64,
}

impl RunOutcome {
    pub fn detached(pid: u32) -> Self {
        Self {
            ok: true,
            text: String::new(),
            usage: Usage::default(),
            runner_session: None,
            exit_code: None,
            error: None,
            pid: Some(pid),
            duration_ms: 0,
        }
    }

    pub fn failure(error: impl Into<String>, exit_code: Option<i32>) -> Self {
        Self {
            ok: false,
            text: String::new(),
            usage: Usage::default(),
            runner_session: None,
            exit_code,
            error: Some(error.into()),
            pid: None,
            duration_ms: 0,
        }
    }
}

/// What a particular harness can do.
///
/// Reported rather than assumed, so a use case can degrade honestly: if a
/// runner cannot resume a session, a heartbeat says "starting a fresh turn"
/// instead of silently losing the thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerCapabilities {
    pub name: String,
    pub can_resume: bool,
    pub can_stream: bool,
    pub reports_usage: bool,
    pub supports_system_prompt: bool,
    pub supports_model_selection: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_needs_a_prompt() {
        assert!(RunRequest::new("ses-1", "  \n ", "/tmp").is_err());
        assert!(RunRequest::new("ses-1", "do the thing", "/tmp").is_ok());
    }
}
