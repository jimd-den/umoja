use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};
use crate::timespec::Interval;

/// How a prompt is delivered to a session that may already be busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryMode {
    /// Steer a busy target, deliver straight to an idle one.
    Auto,
    /// Interrupt active work on purpose.
    Steer,
    /// Wait until the current turn finishes.
    FollowUp,
}

impl DeliveryMode {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "auto" => Ok(Self::Auto),
            "steer" => Ok(Self::Steer),
            "follow_up" | "followup" => Ok(Self::FollowUp),
            other => Err(DomainError::invalid(format!(
                "unknown delivery mode '{other}'; expected auto, steer or follow-up"
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Steer => "steer",
            Self::FollowUp => "follow-up",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatOwner {
    /// The one visible recurring instruction a person set with `heartbeat set`.
    User,
    /// Created programmatically by the agent. There may be many.
    Agent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatStatus {
    Active,
    Paused,
}

/// A recurring instruction that re-enters a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heartbeat {
    pub id: String,
    pub session_id: String,
    pub owner: HeartbeatOwner,
    /// Free label, used by the agent to find its own heartbeats again.
    pub label: Option<String>,
    pub prompt: String,
    pub interval: Interval,
    pub delivery: DeliveryMode,
    pub status: HeartbeatStatus,
    pub created_at: DateTime<Utc>,
    pub next_fire_at: DateTime<Utc>,
    pub last_fired_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub fire_count: u64,
}

impl Heartbeat {
    pub fn new(
        id: impl Into<String>,
        session_id: impl Into<String>,
        owner: HeartbeatOwner,
        prompt: &str,
        interval: Interval,
        delivery: DeliveryMode,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(DomainError::invalid("a heartbeat needs a prompt"));
        }
        Ok(Self {
            id: id.into(),
            session_id: session_id.into(),
            owner,
            label: None,
            prompt: prompt.to_string(),
            interval,
            delivery,
            status: HeartbeatStatus::Active,
            created_at: now,
            next_fire_at: now + interval.to_duration(),
            last_fired_at: None,
            fire_count: 0,
        })
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.status == HeartbeatStatus::Active && now >= self.next_fire_at
    }

    /// Marks a firing and schedules the next one.
    ///
    /// The next time is computed forward from *now*, never by adding an
    /// interval to a stale deadline — a worker that was asleep for an hour owes
    /// one heartbeat, not twelve.
    pub fn mark_fired(&mut self, now: DateTime<Utc>) {
        self.last_fired_at = Some(now);
        self.fire_count += 1;
        self.next_fire_at = now + self.interval.to_duration();
    }

    pub fn pause(&mut self) {
        self.status = HeartbeatStatus::Paused;
    }

    pub fn resume(&mut self, now: DateTime<Utc>) {
        self.status = HeartbeatStatus::Active;
        self.next_fire_at = now + self.interval.to_duration();
    }

    /// The user's single visible heartbeat is theirs; the agent's Python-side
    /// equivalent must not be able to silence it.
    pub fn is_agent_writable(&self) -> bool {
        self.owner == HeartbeatOwner::Agent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn beat() -> Heartbeat {
        Heartbeat::new(
            "hb-1",
            "ses-1",
            HeartbeatOwner::Agent,
            "check the deployment",
            Interval::parse("10m").unwrap(),
            DeliveryMode::Auto,
            t("2026-08-16T12:00:00Z"),
        )
        .unwrap()
    }

    #[test]
    fn missed_ticks_coalesce_into_one() {
        let mut hb = beat();
        assert!(hb.is_due(t("2026-08-16T13:00:00Z")));
        hb.mark_fired(t("2026-08-16T13:00:00Z"));
        // An hour late, but only one debt: the next fire is 10m from now.
        assert_eq!(hb.next_fire_at, t("2026-08-16T13:10:00Z"));
        assert_eq!(hb.fire_count, 1);
    }

    #[test]
    fn a_paused_heartbeat_is_never_due() {
        let mut hb = beat();
        hb.pause();
        assert!(!hb.is_due(t("2026-08-17T00:00:00Z")));
        hb.resume(t("2026-08-17T00:00:00Z"));
        assert!(!hb.is_due(t("2026-08-17T00:05:00Z")));
        assert!(hb.is_due(t("2026-08-17T00:10:00Z")));
    }

    #[test]
    fn the_user_heartbeat_is_not_agent_writable() {
        let mut hb = beat();
        hb.owner = HeartbeatOwner::User;
        assert!(!hb.is_agent_writable());
    }
}
