//! The append-only record of what happened in a session.
//!
//! One JSONL line per event. Append-only is not a storage preference: a
//! transcript that can be rewritten cannot be used to justify a refinement, and
//! evidence is the thing this whole system asks for before it remembers
//! anything.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::session::Usage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEvent {
    SessionStarted {
        name: String,
        runner: String,
        workdir: String,
        model: Option<String>,
    },
    UserPrompt {
        text: String,
    },
    AssistantTurn {
        text: String,
        usage: Usage,
    },
    KernelExec {
        code: String,
        ok: bool,
        duration_ms: u64,
    },
    SubagentAdmitted {
        child_id: String,
        name: String,
        model: String,
    },
    SubagentSettled {
        child_id: String,
        status: String,
    },
    /// Prime Agent's `child_usage_attributed`: the child's cost folded into the
    /// parent turn that launched it, recorded so a reload cannot double-count.
    ChildUsageAttributed {
        child_id: String,
        child_usage: Usage,
        aggregate: Usage,
    },
    MessageSent {
        message_id: String,
        receiver: String,
        status: String,
    },
    MessageReceived {
        message_id: String,
        sender: String,
    },
    HeartbeatFired {
        heartbeat_id: String,
        prompt: String,
    },
    ScheduleFired {
        job_id: String,
        prompt: String,
    },
    GoalChanged {
        status: String,
        objective: String,
    },
    GateRan {
        command: String,
        passed: bool,
        exit_code: Option<i32>,
    },
    AutonomousDecision {
        decision: String,
        reason: String,
    },
    Compacted {
        trigger: String,
        freed_tokens: u64,
    },
    Refined {
        refinement_id: String,
        op: String,
        summary: String,
    },
    SessionEnded {
        status: String,
    },
    Error {
        context: String,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptRecord {
    pub session_id: String,
    pub at: DateTime<Utc>,
    #[serde(flatten)]
    pub event: TranscriptEvent,
}

impl TranscriptRecord {
    pub fn new(session_id: impl Into<String>, at: DateTime<Utc>, event: TranscriptEvent) -> Self {
        Self {
            session_id: session_id.into(),
            at,
            event,
        }
    }

    /// A short human line, for `pa session log`.
    pub fn summary(&self) -> String {
        match &self.event {
            TranscriptEvent::SessionStarted { name, runner, .. } => {
                format!("session {name} started on {runner}")
            }
            TranscriptEvent::UserPrompt { text } => format!("user: {}", clip(text, 72)),
            TranscriptEvent::AssistantTurn { text, usage } => {
                format!("assistant ({} tok): {}", usage.total_tokens(), clip(text, 60))
            }
            TranscriptEvent::KernelExec { ok, duration_ms, .. } => {
                format!("kernel exec {} in {duration_ms}ms", if *ok { "ok" } else { "failed" })
            }
            TranscriptEvent::SubagentAdmitted { name, model, .. } => {
                format!("admitted child {name} on {model}")
            }
            TranscriptEvent::SubagentSettled { child_id, status } => {
                format!("child {child_id} settled: {status}")
            }
            TranscriptEvent::ChildUsageAttributed { child_id, child_usage, .. } => format!(
                "attributed {} tokens from {child_id}",
                child_usage.total_tokens()
            ),
            TranscriptEvent::MessageSent { receiver, status, .. } => {
                format!("message to {receiver}: {status}")
            }
            TranscriptEvent::MessageReceived { sender, .. } => format!("message from {sender}"),
            TranscriptEvent::HeartbeatFired { prompt, .. } => {
                format!("heartbeat: {}", clip(prompt, 60))
            }
            TranscriptEvent::ScheduleFired { prompt, .. } => {
                format!("schedule: {}", clip(prompt, 60))
            }
            TranscriptEvent::GoalChanged { status, objective } => {
                format!("goal {status}: {}", clip(objective, 56))
            }
            TranscriptEvent::GateRan { command, passed, .. } => {
                format!("gate {} {command}", if *passed { "passed" } else { "failed" })
            }
            TranscriptEvent::AutonomousDecision { decision, reason } => {
                format!("autonomous {decision}: {reason}")
            }
            TranscriptEvent::Compacted { trigger, freed_tokens } => {
                format!("compacted on {trigger}, freed {freed_tokens} tokens")
            }
            TranscriptEvent::Refined { op, summary, .. } => format!("refine {op}: {summary}"),
            TranscriptEvent::SessionEnded { status } => format!("session ended: {status}"),
            TranscriptEvent::Error { context, detail } => format!("error in {context}: {detail}"),
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= max {
        return flat;
    }
    flat.chars().take(max.saturating_sub(3)).chain("...".chars()).collect()
}
