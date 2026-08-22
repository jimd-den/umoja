//! Context compaction.
//!
//! Compaction is not a completion signal. It summarises old messages and keeps
//! recent ones so a long task can continue — goals, heartbeats, autonomous
//! continuations and child sessions all survive it untouched. The kernel
//! namespace survives it too, which is precisely why loading data into a
//! variable beats printing it into the transcript.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompactionTrigger {
    /// The context window was exceeded.
    Overflow,
    /// The configured fraction of the window was crossed.
    Threshold,
    /// Asked for explicitly.
    Manual,
}

impl CompactionTrigger {
    pub fn label(self) -> &'static str {
        match self {
            Self::Overflow => "overflow",
            Self::Threshold => "threshold",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactionState {
    pub session_id: String,
    /// Fraction of the window at which compaction becomes due.
    pub threshold: f32,
    pub context_window: u64,
    pub used_tokens: u64,
    /// Messages always kept verbatim, however full the window is.
    pub keep_recent_messages: u32,
    #[serde(default)]
    pub compactions: u32,
    pub last_compacted_at: Option<DateTime<Utc>>,
}

impl CompactionState {
    pub fn new(session_id: impl Into<String>, context_window: u64) -> Self {
        Self {
            session_id: session_id.into(),
            threshold: 0.85,
            context_window,
            used_tokens: 0,
            keep_recent_messages: 12,
            compactions: 0,
            last_compacted_at: None,
        }
    }

    pub fn utilisation(&self) -> f32 {
        if self.context_window == 0 {
            return 0.0;
        }
        self.used_tokens as f32 / self.context_window as f32
    }

    pub fn is_due(&self) -> bool {
        self.utilisation() >= self.threshold
    }

    pub fn plan(&self, trigger: CompactionTrigger, instruction: Option<String>) -> CompactionPlan {
        CompactionPlan {
            session_id: self.session_id.clone(),
            trigger,
            keep_recent_messages: self.keep_recent_messages,
            instruction,
        }
    }

    pub fn record(&mut self, freed_tokens: u64, now: DateTime<Utc>) {
        self.used_tokens = self.used_tokens.saturating_sub(freed_tokens);
        self.compactions += 1;
        self.last_compacted_at = Some(now);
    }
}

/// What a compaction will do, decided before anything is thrown away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactionPlan {
    pub session_id: String,
    pub trigger: CompactionTrigger,
    pub keep_recent_messages: u32,
    /// What the summary must preserve — "keep the failing tests and the
    /// remaining migration steps".
    pub instruction: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compaction_is_due_at_the_threshold() {
        let mut state = CompactionState::new("ses-1", 200_000);
        state.used_tokens = 100_000;
        assert!(!state.is_due());
        state.used_tokens = 170_000;
        assert!(state.is_due());
    }

    #[test]
    fn recording_frees_tokens_without_underflowing() {
        let mut state = CompactionState::new("ses-1", 200_000);
        state.used_tokens = 1_000;
        state.record(
            5_000,
            DateTime::parse_from_rfc3339("2026-08-16T12:00:00Z")
                .unwrap()
                .with_timezone(&Utc),
        );
        assert_eq!(state.used_tokens, 0);
        assert_eq!(state.compactions, 1);
    }

    #[test]
    fn a_zero_window_does_not_divide_by_zero() {
        let state = CompactionState::new("ses-1", 0);
        assert_eq!(state.utilisation(), 0.0);
        assert!(!state.is_due());
    }
}
