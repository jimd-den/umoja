//! Bounded autonomous continuation.
//!
//! Autonomous mode is a *policy*, not an engine: given what has happened and
//! what the gates say, may the session continue, and if not, why not. Keeping
//! it a pure decision means the interesting cases — a gate that keeps failing
//! on an unchanged workspace, a token limit reached mid-gate — are testable
//! without running a single command.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};
use crate::session::Usage;

/// A shell command that must pass before the run may finish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gate {
    pub command: String,
    /// Output beyond this is clipped before being handed back to the agent, so
    /// a 200k-line test log cannot evict the task from context.
    #[serde(default = "default_gate_output_limit")]
    pub max_output_bytes: usize,
}

fn default_gate_output_limit() -> usize {
    8 * 1024
}

impl Gate {
    pub fn new(command: impl Into<String>) -> Result<Self> {
        let command = command.into().trim().to_string();
        if command.is_empty() {
            return Err(DomainError::invalid("a gate needs a command"));
        }
        Ok(Self {
            command,
            max_output_bytes: default_gate_output_limit(),
        })
    }
}

/// What running a gate produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateOutcome {
    pub command: String,
    pub passed: bool,
    pub exit_code: Option<i32>,
    /// Already clipped to the gate's limit.
    pub output: String,
    /// A fingerprint of the workspace when this gate ran. Re-running a failed
    /// gate against an unchanged workspace can only fail again.
    pub workspace_fingerprint: Option<String>,
    pub ran_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomousLimits {
    pub max_continuations: u32,
    pub max_turns: u32,
    pub max_tokens: u64,
    pub max_wall_clock_secs: u64,
}

impl Default for AutonomousLimits {
    fn default() -> Self {
        Self {
            max_continuations: 10,
            max_turns: 20,
            max_tokens: 500_000,
            max_wall_clock_secs: 3_600,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomousPolicy {
    pub enabled: bool,
    #[serde(default)]
    pub gates: Vec<Gate>,
    #[serde(default)]
    pub limits: AutonomousLimits,
}

/// Live counters for one autonomous run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutonomousState {
    pub session_id: String,
    pub policy: AutonomousPolicy,
    #[serde(default)]
    pub continuations: u32,
    #[serde(default)]
    pub turns: u32,
    #[serde(default)]
    pub usage: Usage,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// The most recent result for each gate, by command.
    #[serde(default)]
    pub last_gate_outcomes: Vec<GateOutcome>,
}

/// The decision, and the reason for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Continuation {
    /// Inject another turn.
    Continue { reason: String },
    /// The gates pass and nothing is outstanding.
    Finish { reason: String },
    /// A configured limit was hit. Distinct from `Finish`: the work is not
    /// necessarily done, and saying otherwise would be a lie.
    Stop { reason: String },
}

impl Continuation {
    pub fn should_continue(&self) -> bool {
        matches!(self, Self::Continue { .. })
    }

    pub fn reason(&self) -> &str {
        match self {
            Self::Continue { reason } | Self::Finish { reason } | Self::Stop { reason } => reason,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Continue { .. } => "continue",
            Self::Finish { .. } => "finish",
            Self::Stop { .. } => "stop",
        }
    }
}

impl AutonomousState {
    pub fn new(session_id: impl Into<String>, policy: AutonomousPolicy, now: DateTime<Utc>) -> Self {
        Self {
            session_id: session_id.into(),
            policy,
            continuations: 0,
            turns: 0,
            usage: Usage::default(),
            started_at: now,
            updated_at: now,
            last_gate_outcomes: Vec::new(),
        }
    }

    pub fn record_turn(&mut self, usage: &Usage, now: DateTime<Utc>) {
        self.turns += 1;
        self.usage.absorb(usage);
        self.updated_at = now;
    }

    pub fn record_continuation(&mut self, now: DateTime<Utc>) {
        self.continuations += 1;
        self.updated_at = now;
    }

    pub fn record_gate(&mut self, outcome: GateOutcome) {
        self.last_gate_outcomes
            .retain(|prior| prior.command != outcome.command);
        self.last_gate_outcomes.push(outcome);
    }

    pub fn elapsed_secs(&self, now: DateTime<Utc>) -> u64 {
        (now - self.started_at).num_seconds().max(0) as u64
    }

    /// Would re-running this gate tell us anything new?
    ///
    /// A gate that failed against a workspace fingerprint identical to the
    /// current one will fail identically. Prime Agent avoids that rerun, and so
    /// does this.
    pub fn gate_is_stale(&self, command: &str, fingerprint: Option<&str>) -> bool {
        let Some(prior) = self
            .last_gate_outcomes
            .iter()
            .find(|outcome| outcome.command == command)
        else {
            return false;
        };
        !prior.passed
            && fingerprint.is_some()
            && prior.workspace_fingerprint.as_deref() == fingerprint
    }

    /// The whole of autonomous mode's judgement.
    pub fn decide(&self, now: DateTime<Utc>) -> Continuation {
        if !self.policy.enabled {
            return Continuation::Finish {
                reason: "autonomous mode is off".into(),
            };
        }

        let limits = self.policy.limits;
        if self.continuations >= limits.max_continuations {
            return Continuation::Stop {
                reason: format!(
                    "continuation limit reached ({} of {})",
                    self.continuations, limits.max_continuations
                ),
            };
        }
        if self.turns >= limits.max_turns {
            return Continuation::Stop {
                reason: format!("turn limit reached ({} of {})", self.turns, limits.max_turns),
            };
        }
        let tokens = self.usage.total_tokens();
        if tokens >= limits.max_tokens {
            return Continuation::Stop {
                reason: format!("token limit reached ({tokens} of {})", limits.max_tokens),
            };
        }
        let elapsed = self.elapsed_secs(now);
        if elapsed >= limits.max_wall_clock_secs {
            return Continuation::Stop {
                reason: format!(
                    "time limit reached ({elapsed}s of {}s)",
                    limits.max_wall_clock_secs
                ),
            };
        }

        // Limits are checked before gates on purpose: a run that is out of
        // budget should say so, not spend more of it running a test suite.
        let failing: Vec<&GateOutcome> = self
            .policy
            .gates
            .iter()
            .filter_map(|gate| {
                self.last_gate_outcomes
                    .iter()
                    .find(|outcome| outcome.command == gate.command)
            })
            .filter(|outcome| !outcome.passed)
            .collect();

        if let Some(first) = failing.first() {
            return Continuation::Continue {
                reason: format!("gate failed: {}", first.command),
            };
        }

        let unrun: Vec<&Gate> = self
            .policy
            .gates
            .iter()
            .filter(|gate| {
                !self
                    .last_gate_outcomes
                    .iter()
                    .any(|outcome| outcome.command == gate.command)
            })
            .collect();

        if let Some(gate) = unrun.first() {
            return Continuation::Continue {
                reason: format!("gate not yet run: {}", gate.command),
            };
        }

        Continuation::Finish {
            reason: if self.policy.gates.is_empty() {
                "no gates configured and no limit reached".into()
            } else {
                "all gates passed".into()
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn state(gates: Vec<Gate>) -> AutonomousState {
        AutonomousState::new(
            "ses-1",
            AutonomousPolicy {
                enabled: true,
                gates,
                limits: AutonomousLimits::default(),
            },
            t("2026-08-16T12:00:00Z"),
        )
    }

    fn outcome(command: &str, passed: bool, fingerprint: Option<&str>) -> GateOutcome {
        GateOutcome {
            command: command.into(),
            passed,
            exit_code: Some(if passed { 0 } else { 1 }),
            output: String::new(),
            workspace_fingerprint: fingerprint.map(str::to_string),
            ran_at: t("2026-08-16T12:05:00Z"),
        }
    }

    #[test]
    fn a_failing_gate_buys_another_turn() {
        let mut state = state(vec![Gate::new("npm run check").unwrap()]);
        state.record_gate(outcome("npm run check", false, None));
        let decision = state.decide(t("2026-08-16T12:10:00Z"));
        assert!(decision.should_continue());
        assert!(decision.reason().contains("npm run check"));
    }

    #[test]
    fn passing_gates_finish_the_run() {
        let mut state = state(vec![Gate::new("npm run check").unwrap()]);
        state.record_gate(outcome("npm run check", true, None));
        assert_eq!(state.decide(t("2026-08-16T12:10:00Z")).label(), "finish");
    }

    #[test]
    fn limits_are_a_stop_not_a_finish() {
        let mut state = state(vec![]);
        state.continuations = state.policy.limits.max_continuations;
        let decision = state.decide(t("2026-08-16T12:10:00Z"));
        assert_eq!(decision.label(), "stop");
        assert!(!decision.should_continue());
    }

    #[test]
    fn a_budget_is_checked_before_a_gate_is_rerun() {
        let mut state = state(vec![Gate::new("npm run check").unwrap()]);
        state.record_gate(outcome("npm run check", false, None));
        state.usage.input_tokens = state.policy.limits.max_tokens;
        assert_eq!(state.decide(t("2026-08-16T12:10:00Z")).label(), "stop");
    }

    #[test]
    fn an_unchanged_workspace_makes_a_failed_gate_stale() {
        let mut state = state(vec![Gate::new("npm run check").unwrap()]);
        state.record_gate(outcome("npm run check", false, Some("abc123")));
        assert!(state.gate_is_stale("npm run check", Some("abc123")));
        assert!(!state.gate_is_stale("npm run check", Some("def456")));
    }

    #[test]
    fn recording_a_gate_replaces_its_previous_result() {
        let mut state = state(vec![Gate::new("check").unwrap()]);
        state.record_gate(outcome("check", false, None));
        state.record_gate(outcome("check", true, None));
        assert_eq!(state.last_gate_outcomes.len(), 1);
        assert!(state.last_gate_outcomes[0].passed);
    }

    #[test]
    fn a_disabled_policy_never_continues() {
        let mut state = state(vec![]);
        state.policy.enabled = false;
        assert!(!state.decide(t("2026-08-16T12:10:00Z")).should_continue());
    }
}
