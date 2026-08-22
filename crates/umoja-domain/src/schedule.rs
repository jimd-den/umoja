use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};
use crate::heartbeat::DeliveryMode;
use crate::timespec::CronExpr;

/// When a job runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ScheduleSpec {
    /// Fires once, then completes.
    OneTime { at: DateTime<Utc> },
    /// Fires on every matching minute, forever.
    Cron { expr: CronExpr },
}

impl ScheduleSpec {
    /// Parses `in 30m`, an RFC 3339 instant, or a five-field cron expression.
    pub fn parse(text: &str, now: DateTime<Utc>) -> Result<Self> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err(DomainError::invalid("a schedule needs a time"));
        }

        if let Some(rest) = trimmed.strip_prefix("in ") {
            let interval = crate::timespec::Interval::parse(rest)?;
            return Ok(Self::OneTime {
                at: now + interval.to_duration(),
            });
        }

        if let Ok(at) = DateTime::parse_from_rfc3339(trimmed) {
            return Ok(Self::OneTime {
                at: at.with_timezone(&Utc),
            });
        }

        if trimmed.split_whitespace().count() == 5 {
            return Ok(Self::Cron {
                expr: CronExpr::parse(trimmed)?,
            });
        }

        // A bare interval is a common slip, and guessing between "in 30m" and
        // "every 30m" would be guessing about the user's intent.
        Err(DomainError::invalid(format!(
            "could not read schedule '{trimmed}'; use \"in 30m\", an RFC 3339 instant, \
             or a cron expression like \"0 9 * * 1-5\""
        )))
    }

    pub fn next_after(&self, after: DateTime<Utc>) -> Option<DateTime<Utc>> {
        match self {
            Self::OneTime { at } => (*at > after).then_some(*at),
            Self::Cron { expr } => expr.next_after(after),
        }
    }

    pub fn is_recurring(&self) -> bool {
        matches!(self, Self::Cron { .. })
    }

    pub fn describe(&self) -> String {
        match self {
            Self::OneTime { at } => format!("once at {}", at.to_rfc3339()),
            Self::Cron { expr } => format!("cron {expr}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Pending,
    /// A tick has been claimed and is being delivered. Claiming before
    /// delivering is what stops a crash from replaying an uncertain prompt.
    Claimed,
    Completed,
    Cancelled,
    Failed,
}

/// A prompt aimed at a session at a time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduledJob {
    pub id: String,
    /// Session id or name. Resolved at delivery so a renamed agent keeps its
    /// schedule.
    pub target: String,
    pub prompt: String,
    pub spec: ScheduleSpec,
    pub delivery: DeliveryMode,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    pub next_tick: Option<DateTime<Utc>>,
    pub last_tick: Option<DateTime<Utc>>,
    #[serde(default)]
    pub run_count: u64,
    pub last_error: Option<String>,
}

impl ScheduledJob {
    pub fn new(
        id: impl Into<String>,
        target: impl Into<String>,
        prompt: &str,
        spec: ScheduleSpec,
        delivery: DeliveryMode,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err(DomainError::invalid("a scheduled job needs a prompt"));
        }
        let next_tick = spec.next_after(now - chrono::Duration::seconds(1));
        if next_tick.is_none() {
            return Err(DomainError::invalid(
                "that schedule will never fire; check the date or cron expression",
            ));
        }
        Ok(Self {
            id: id.into(),
            target: target.into(),
            prompt: prompt.to_string(),
            spec,
            delivery,
            status: JobStatus::Pending,
            created_at: now,
            next_tick,
            last_tick: None,
            run_count: 0,
            last_error: None,
        })
    }

    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.status == JobStatus::Pending
            && self.next_tick.is_some_and(|tick| now >= tick)
    }

    /// Takes ownership of the due tick. Returns an error rather than a silent
    /// no-op, because two workers claiming the same tick is a bug worth seeing.
    pub fn claim(&mut self, now: DateTime<Utc>) -> Result<()> {
        if !self.is_due(now) {
            return Err(DomainError::forbidden(format!(
                "job {} is not due",
                self.id
            )));
        }
        self.status = JobStatus::Claimed;
        self.last_tick = Some(now);
        Ok(())
    }

    /// Records a delivered tick and either reschedules or finishes.
    pub fn complete_tick(&mut self, now: DateTime<Utc>) {
        self.run_count += 1;
        self.last_error = None;
        match self.spec.next_after(now) {
            Some(next) if self.spec.is_recurring() => {
                self.next_tick = Some(next);
                self.status = JobStatus::Pending;
            }
            _ => {
                self.next_tick = None;
                self.status = JobStatus::Completed;
            }
        }
    }

    /// A failed delivery keeps a recurring job alive — the next tick may well
    /// work — but retires a one-time job that can no longer be delivered.
    pub fn fail_tick(&mut self, reason: impl Into<String>, now: DateTime<Utc>) {
        self.last_error = Some(reason.into());
        match self.spec.next_after(now) {
            Some(next) if self.spec.is_recurring() => {
                self.next_tick = Some(next);
                self.status = JobStatus::Pending;
            }
            _ => {
                self.next_tick = None;
                self.status = JobStatus::Failed;
            }
        }
    }

    pub fn cancel(&mut self) {
        self.status = JobStatus::Cancelled;
        self.next_tick = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn relative_schedules_resolve_against_now() {
        let now = t("2026-08-16T12:00:00Z");
        let spec = ScheduleSpec::parse("in 30m", now).unwrap();
        assert_eq!(
            spec.next_after(now),
            Some(t("2026-08-16T12:30:00Z"))
        );
    }

    #[test]
    fn a_bare_interval_is_refused_rather_than_guessed() {
        let now = t("2026-08-16T12:00:00Z");
        assert!(ScheduleSpec::parse("30m", now).is_err());
    }

    #[test]
    fn one_time_jobs_complete_and_cron_jobs_requeue() {
        let now = t("2026-08-16T12:00:00Z");
        let mut once = ScheduledJob::new(
            "job-1",
            "worker",
            "check the benchmark",
            ScheduleSpec::parse("in 30m", now).unwrap(),
            DeliveryMode::Auto,
            now,
        )
        .unwrap();

        let fire = t("2026-08-16T12:30:00Z");
        assert!(once.is_due(fire));
        once.claim(fire).unwrap();
        once.complete_tick(fire);
        assert_eq!(once.status, JobStatus::Completed);
        assert!(once.next_tick.is_none());

        let mut cron = ScheduledJob::new(
            "job-2",
            "worker",
            "review open work",
            ScheduleSpec::parse("0 9 * * 1-5", now).unwrap(),
            DeliveryMode::FollowUp,
            now,
        )
        .unwrap();
        let fire = t("2026-08-17T09:00:00Z");
        cron.claim(fire).unwrap();
        cron.complete_tick(fire);
        assert_eq!(cron.status, JobStatus::Pending);
        assert_eq!(cron.next_tick, Some(t("2026-08-18T09:00:00Z")));
    }

    #[test]
    fn a_tick_cannot_be_claimed_twice() {
        let now = t("2026-08-16T12:00:00Z");
        let mut job = ScheduledJob::new(
            "job-1",
            "worker",
            "go",
            ScheduleSpec::parse("in 1m", now).unwrap(),
            DeliveryMode::Auto,
            now,
        )
        .unwrap();
        let fire = t("2026-08-16T12:01:00Z");
        job.claim(fire).unwrap();
        assert!(job.claim(fire).is_err());
    }

    #[test]
    fn impossible_schedules_are_refused_at_creation() {
        let now = t("2026-08-16T12:00:00Z");
        let spec = ScheduleSpec::parse("2020-01-01T00:00:00Z", now).unwrap();
        assert!(ScheduledJob::new("job-1", "w", "go", spec, DeliveryMode::Auto, now).is_err());
    }
}
