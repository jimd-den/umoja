use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};
use crate::heartbeat::DeliveryMode;

/// Who, relative to the sender, is being addressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReceiverRole {
    Parent,
    Child,
    Sibling,
    /// Any addressable session on this machine, named directly.
    Peer,
    /// Everyone in the sender's family: parent, children, siblings. Never the
    /// whole machine — a broadcast that can reach unrelated work is a footgun.
    Broadcast,
}

impl ReceiverRole {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "parent" => Ok(Self::Parent),
            "child" => Ok(Self::Child),
            "sibling" => Ok(Self::Sibling),
            "peer" | "agent" => Ok(Self::Peer),
            "all" | "broadcast" => Ok(Self::Broadcast),
            other => Err(DomainError::invalid(format!(
                "unknown receiver role '{other}'; expected parent, child, sibling, peer or all"
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Sibling => "sibling",
            Self::Peer => "peer",
            Self::Broadcast => "all",
        }
    }

    pub fn needs_name(self) -> bool {
        matches!(self, Self::Child | Self::Sibling | Self::Peer)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStatus {
    /// Reached an idle target's context immediately.
    Delivered,
    /// Accepted for later delivery to a busy target.
    Queued,
    /// Read by the receiving session.
    Consumed,
    Failed,
}

impl DeliveryStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Queued => "queued",
            Self::Consumed => "consumed",
            Self::Failed => "failed",
        }
    }
}

/// Limits the bus enforces so one chatty agent cannot drown another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MessageLimits {
    pub max_body_bytes: usize,
    pub max_pending_per_target: usize,
}

impl Default for MessageLimits {
    fn default() -> Self {
        Self {
            max_body_bytes: 64 * 1024,
            max_pending_per_target: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessage {
    pub id: String,
    pub sender_session_id: String,
    pub sender_name: String,
    pub receiver_role: ReceiverRole,
    /// Resolved target session id. Set at accept time so a rename cannot
    /// misroute a message that was already queued.
    pub receiver_session_id: String,
    pub receiver_name: String,
    pub body: String,
    pub mode: DeliveryMode,
    pub status: DeliveryStatus,
    pub created_at: DateTime<Utc>,
    pub delivered_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl AgentMessage {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        sender_session_id: impl Into<String>,
        sender_name: impl Into<String>,
        receiver_role: ReceiverRole,
        receiver_session_id: impl Into<String>,
        receiver_name: impl Into<String>,
        body: &str,
        mode: DeliveryMode,
        limits: MessageLimits,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let body = body.trim();
        if body.is_empty() {
            return Err(DomainError::invalid("a message needs a body"));
        }
        if body.len() > limits.max_body_bytes {
            return Err(DomainError::LimitReached {
                limit: "message size",
                reached: body.len() as u64,
                allowed: limits.max_body_bytes as u64,
            });
        }
        Ok(Self {
            id: id.into(),
            sender_session_id: sender_session_id.into(),
            sender_name: sender_name.into(),
            receiver_role,
            receiver_session_id: receiver_session_id.into(),
            receiver_name: receiver_name.into(),
            body: body.to_string(),
            mode,
            status: DeliveryStatus::Queued,
            created_at: now,
            delivered_at: None,
            error: None,
        })
    }

    pub fn mark(&mut self, status: DeliveryStatus, now: DateTime<Utc>) {
        self.status = status;
        if matches!(status, DeliveryStatus::Delivered | DeliveryStatus::Consumed) {
            self.delivered_at = Some(now);
        }
    }

    pub fn is_pending(&self) -> bool {
        matches!(self.status, DeliveryStatus::Queued | DeliveryStatus::Delivered)
    }
}

/// What the sender gets back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Receipt {
    pub message_id: String,
    pub receiver_name: String,
    pub delivery_status: DeliveryStatus,
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn message(body: &str) -> Result<AgentMessage> {
        AgentMessage::new(
            "msg-1",
            "ses-1",
            "root",
            ReceiverRole::Child,
            "ses-2",
            "api-reviewer",
            body,
            DeliveryMode::Auto,
            MessageLimits::default(),
            now(),
        )
    }

    #[test]
    fn empty_and_oversized_bodies_are_refused() {
        assert!(message("   ").is_err());
        let huge = "x".repeat(64 * 1024 + 1);
        assert!(matches!(
            message(&huge),
            Err(DomainError::LimitReached { .. })
        ));
    }

    #[test]
    fn roles_that_name_a_target_say_so() {
        assert!(ReceiverRole::Child.needs_name());
        assert!(!ReceiverRole::Parent.needs_name());
        assert!(!ReceiverRole::Broadcast.needs_name());
    }

    #[test]
    fn delivery_stamps_a_time_only_when_it_arrives() {
        let mut msg = message("hello").unwrap();
        assert!(msg.delivered_at.is_none());
        msg.mark(DeliveryStatus::Delivered, now());
        assert!(msg.delivered_at.is_some());
    }
}
