use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    /// Attached or working.
    Running,
    /// Alive, nothing in flight. The normal state of a detached worker.
    Idle,
    /// Finished and kept for its transcript.
    Completed,
    Cancelled,
    Failed,
}

impl SessionStatus {
    pub fn is_live(self) -> bool {
        matches!(self, Self::Running | Self::Idle)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Idle => "idle",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// Started by a person.
    Root,
    /// Admitted by a parent through `agent spawn`.
    Child,
}

/// One agent session: a transcript, a working directory, and whatever durable
/// state (kernel, goal, schedules, children) hangs off it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    /// A stable readable handle. Every command that takes an agent accepts
    /// either this or the id.
    pub name: String,
    pub kind: SessionKind,
    pub status: SessionStatus,
    pub workdir: String,
    /// The harness this session runs under (`claude`, `opencode`, ...), so a
    /// resumed session goes back to the runner that understands it.
    pub runner: String,
    pub model: Option<String>,
    pub parent_id: Option<String>,
    /// 0 for a root session; children may not exceed the configured maximum.
    pub depth: u8,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Wall-clock and token totals, including usage attributed from children.
    #[serde(default)]
    pub usage: Usage,
    /// OS process id while a worker is resident.
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub turns: u64,
    /// Tokens spent by children and folded into this session's totals. Kept
    /// separate so "own usage" stays recoverable, exactly as prime-agent's
    /// context-tree reporting requires.
    #[serde(default)]
    pub attributed_child_tokens: u64,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }

    pub fn own_tokens(&self) -> u64 {
        self.total_tokens()
            .saturating_sub(self.attributed_child_tokens)
    }

    pub fn absorb(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.turns += other.turns;
    }

    pub fn attribute_child(&mut self, child: &Usage) {
        let tokens = child.total_tokens();
        self.input_tokens += child.input_tokens;
        self.output_tokens += child.output_tokens;
        self.attributed_child_tokens += tokens;
    }
}

impl Session {
    /// Session names are addresses, so they are normalised the way a hostname
    /// would be: lowercase, hyphenated, no surprises when typed twice.
    pub fn normalise_name(raw: &str) -> Result<String> {
        let mut out = String::new();
        let mut last_dash = true;
        for ch in raw.trim().chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
                last_dash = false;
            } else if !last_dash {
                out.push('-');
                last_dash = true;
            }
        }
        let name = out.trim_matches('-').to_string();
        if name.is_empty() {
            return Err(DomainError::invalid(
                "a session name needs at least one letter or digit",
            ));
        }
        if name.len() > 64 {
            return Err(DomainError::invalid("session names are 64 characters or less"));
        }
        Ok(name)
    }

    pub fn matches_selector(&self, selector: &str) -> bool {
        self.id == selector || self.name == selector
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_normalise_to_addresses() {
        assert_eq!(Session::normalise_name("API Reviewer").unwrap(), "api-reviewer");
        assert_eq!(Session::normalise_name("  __weird__  ").unwrap(), "weird");
        assert_eq!(Session::normalise_name("a/b/c").unwrap(), "a-b-c");
    }

    #[test]
    fn empty_names_are_rejected() {
        assert!(Session::normalise_name("   ").is_err());
        assert!(Session::normalise_name("///").is_err());
    }

    #[test]
    fn child_usage_stays_separable() {
        let mut parent = Usage {
            input_tokens: 100,
            output_tokens: 50,
            turns: 1,
            attributed_child_tokens: 0,
        };
        parent.attribute_child(&Usage {
            input_tokens: 20,
            output_tokens: 10,
            turns: 1,
            attributed_child_tokens: 0,
        });
        assert_eq!(parent.total_tokens(), 180);
        assert_eq!(parent.own_tokens(), 150);
    }
}
