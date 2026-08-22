//! Recursive delegation — prime-agent's `rlm(...)`.
//!
//! The invariant reproduced here is the important one: **a spawn returns an
//! admission handle, never an answer**. The handle proves a child was admitted
//! and says where to find it. Results come back later as messages or files. A
//! parent that blocks on a child is a parent that cannot do anything else, and
//! it is the difference between delegation and a function call.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};
use crate::session::Usage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Admitted, not yet started by the runner.
    Admitted,
    Running,
    Completed,
    Failed,
    Cancelled,
    /// Deleted by the parent. The registry keeps a tombstone; the transcript on
    /// disk is left alone.
    Deleted,
}

impl SubagentStatus {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Admitted | Self::Running)
    }

    /// Can the parent still send this child a message?
    pub fn is_addressable(self) -> bool {
        matches!(self, Self::Admitted | Self::Running | Self::Completed)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Deleted => "deleted",
        }
    }
}

/// What comes back the instant a child is admitted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpawnHandle {
    pub child_id: String,
    pub name: String,
    pub session_id: String,
    pub session_dir: String,
    pub model: String,
    pub depth: u8,
}

/// What comes back from a **blocking** delegation.
///
/// [`SpawnHandle`] is deliberately answerless because fan-out is the common
/// case. This is the other one: the parent asked a question whose answer is
/// the input to its next step, and waited for it. Both the text and what it
/// cost come back, so a caller can decide whether the wait was worth it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallResult {
    pub handle: SpawnHandle,
    /// False when the child ran and failed. The text may still hold whatever
    /// it managed to say, so this is what a caller must branch on rather than
    /// on the text being non-empty.
    pub ok: bool,
    pub text: String,
    #[serde(default)]
    pub usage: Usage,
    pub error: Option<String>,
    pub duration_ms: u64,
}

/// A reusable delegation role, as stored in the harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SubagentSpec {
    pub name: String,
    pub prompt: String,
    pub model: Option<String>,
    #[serde(default)]
    pub tools: Vec<String>,
    pub runner: Option<String>,
}

/// A child in the parent-scoped registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subagent {
    pub child_id: String,
    pub parent_session_id: String,
    pub session_id: String,
    pub name: String,
    pub session_dir: String,
    pub model: String,
    pub runner: String,
    pub prompt: String,
    pub depth: u8,
    pub status: SubagentStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub usage: Usage,
    /// Set once the child's usage has been folded into the parent, so a replayed
    /// registry cannot double-charge.
    #[serde(default)]
    pub usage_attributed: bool,
    pub last_error: Option<String>,
}

impl Subagent {
    pub fn handle(&self) -> SpawnHandle {
        SpawnHandle {
            child_id: self.child_id.clone(),
            name: self.name.clone(),
            session_id: self.session_id.clone(),
            session_dir: self.session_dir.clone(),
            model: self.model.clone(),
            depth: self.depth,
        }
    }

    /// A child answers to its child id, its session id or its name.
    pub fn matches_selector(&self, selector: &str) -> bool {
        self.child_id == selector || self.session_id == selector || self.name == selector
    }

    pub fn settle(&mut self, status: SubagentStatus, now: DateTime<Utc>) {
        self.status = status;
        self.updated_at = now;
    }
}

/// The recursion rule, kept in one place so both the caller and the host can
/// apply it and agree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DepthPolicy {
    pub max_depth: u8,
}

impl Default for DepthPolicy {
    /// One, like prime-agent: a root session may create children, and those
    /// children may not create grandchildren unless this is raised.
    fn default() -> Self {
        Self { max_depth: 1 }
    }
}

impl DepthPolicy {
    pub fn admit(&self, parent_depth: u8) -> Result<u8> {
        if parent_depth >= self.max_depth {
            return Err(DomainError::Forbidden(format!(
                "recursion depth {} reached (max {}); raise max-depth to go deeper",
                parent_depth, self.max_depth
            )));
        }
        Ok(parent_depth + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_one_allows_children_but_not_grandchildren() {
        let policy = DepthPolicy::default();
        assert_eq!(policy.admit(0).unwrap(), 1);
        assert!(policy.admit(1).is_err());
    }

    #[test]
    fn raising_the_limit_allows_deeper_recursion() {
        let policy = DepthPolicy { max_depth: 3 };
        assert_eq!(policy.admit(2).unwrap(), 3);
        assert!(policy.admit(3).is_err());
    }

    #[test]
    fn completed_children_stay_addressable() {
        assert!(SubagentStatus::Completed.is_addressable());
        assert!(!SubagentStatus::Completed.is_live());
        assert!(!SubagentStatus::Deleted.is_addressable());
    }
}
