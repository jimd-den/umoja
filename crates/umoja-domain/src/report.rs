//! Reports an agent files about the tools it is using.
//!
//! An agent that hits a broken builtin, a misleading result, or a missing
//! capability has nowhere to put that knowledge: it works around the
//! problem, the session ends, and the next agent rediscovers it.  A report
//! is the durable form of "this tool did the wrong thing", addressed to
//! whoever maintains the tool rather than to the user in the loop.
//!
//! The entity is deliberately small.  What makes a report useful is not
//! structure but *reproduction*, so the invariants enforced here are the
//! ones that separate an actionable report from a shrug: it must say what
//! was expected, and what happened instead.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

/// What kind of trouble is being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportKind {
    /// A tool did something incorrect.
    Bug,
    /// A tool failed outright — a crash, a non-zero exit, a panic.
    Error,
    /// A tool worked, but cost far more effort than it should have.
    Friction,
    /// A capability that would have helped and does not exist.
    Idea,
}

impl ReportKind {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "bug" => Ok(Self::Bug),
            "error" | "crash" => Ok(Self::Error),
            "friction" | "papercut" => Ok(Self::Friction),
            "idea" | "feature" => Ok(Self::Idea),
            other => Err(DomainError::invalid(format!(
                "unknown report kind '{other}'; expected bug, error, friction or idea"
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Bug => "bug",
            Self::Error => "error",
            Self::Friction => "friction",
            Self::Idea => "idea",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportStatus {
    Open,
    Triaged,
    Resolved,
}

impl ReportStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Triaged => "triaged",
            Self::Resolved => "resolved",
        }
    }
}

/// The largest body worth keeping.  A report is a pointer to a problem,
/// not a transcript of the session that found it.
pub const MAX_BODY_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub id: String,
    pub kind: ReportKind,
    /// One line naming the problem.
    pub title: String,
    /// What was expected, what happened, and how to make it happen again.
    pub body: String,
    /// The builtin, command, or subsystem at fault, when it is known.
    pub component: Option<String>,
    /// Who filed it — the agent's own name, so a pattern across sessions
    /// is attributable.
    pub agent: String,
    pub session_id: Option<String>,
    pub status: ReportStatus,
    pub created_at: DateTime<Utc>,
}

impl Report {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: impl Into<String>,
        kind: ReportKind,
        title: &str,
        body: &str,
        component: Option<String>,
        agent: impl Into<String>,
        session_id: Option<String>,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let title = title.trim();
        if title.is_empty() {
            return Err(DomainError::invalid("a report needs a title"));
        }
        if title.len() > 200 {
            return Err(DomainError::invalid(
                "a report title is one line; put the detail in the body",
            ));
        }

        let body = body.trim();
        // A report with no body is a complaint.  The maintainer cannot act
        // on "grep is broken", so the entity refuses to store one.
        if body.is_empty() {
            return Err(DomainError::invalid(
                "a report needs a body saying what was expected and what happened instead",
            ));
        }
        if body.len() > MAX_BODY_BYTES {
            return Err(DomainError::LimitReached {
                limit: "report body",
                reached: body.len() as u64,
                allowed: MAX_BODY_BYTES as u64,
            });
        }

        Ok(Self {
            id: id.into(),
            kind,
            title: title.to_string(),
            body: body.to_string(),
            component: component.filter(|c| !c.trim().is_empty()),
            agent: agent.into(),
            session_id,
            status: ReportStatus::Open,
            created_at: now,
        })
    }

    pub fn is_open(&self) -> bool {
        matches!(self.status, ReportStatus::Open | ReportStatus::Triaged)
    }

    /// The form a maintainer reads: a heading, the provenance, the body.
    pub fn to_markdown(&self) -> String {
        let component = self.component.as_deref().unwrap_or("unspecified");
        let session = self.session_id.as_deref().unwrap_or("-");
        format!(
            "## [{kind}] {title}\n\n\
             - **id**: `{id}`\n\
             - **component**: `{component}`\n\
             - **status**: {status}\n\
             - **filed by**: {agent} (session `{session}`)\n\
             - **when**: {when}\n\n\
             {body}\n",
            kind = self.kind.label(),
            title = self.title,
            id = self.id,
            component = component,
            status = self.status.label(),
            agent = self.agent,
            session = session,
            when = self.created_at.to_rfc3339(),
            body = self.body,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-23T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn report(title: &str, body: &str) -> Result<Report> {
        Report::new(
            "rep-1",
            ReportKind::Bug,
            title,
            body,
            Some("ast_rewrite".into()),
            "claude",
            Some("ses-1".into()),
            now(),
        )
    }

    /// The whole value of a report is that someone else can act on it.
    #[test]
    fn a_report_without_a_reproduction_is_refused() {
        assert!(report("ast_rewrite eats the file", "   ").is_err());
        assert!(report("   ", "expected X, got Y").is_err());
        assert!(report("ast_rewrite eats the file", "expected X, got Y").is_ok());
    }

    #[test]
    fn an_oversized_body_is_refused_rather_than_truncated() {
        let huge = "x".repeat(MAX_BODY_BYTES + 1);
        assert!(matches!(
            report("too much", &huge),
            Err(DomainError::LimitReached { .. })
        ));
    }

    #[test]
    fn kinds_round_trip_through_their_labels() {
        for kind in [
            ReportKind::Bug,
            ReportKind::Error,
            ReportKind::Friction,
            ReportKind::Idea,
        ] {
            assert_eq!(ReportKind::parse(kind.label()).unwrap(), kind);
        }
        assert!(ReportKind::parse("nonsense").is_err());
    }

    #[test]
    fn a_new_report_is_open_and_renders_its_provenance() {
        let r = report("ast_rewrite eats the file", "expected X, got Y").unwrap();
        assert!(r.is_open());
        let md = r.to_markdown();
        assert!(md.contains("[bug] ast_rewrite eats the file"));
        assert!(md.contains("ast_rewrite"));
        assert!(md.contains("ses-1"));
        assert!(md.contains("expected X, got Y"));
    }

    /// A blank component is the same as no component; storing `Some("")`
    /// would put an empty backtick pair in every rendered report.
    #[test]
    fn a_blank_component_is_dropped() {
        let r = Report::new(
            "rep-2",
            ReportKind::Idea,
            "add ast_rewrite",
            "it would help",
            Some("  ".into()),
            "claude",
            None,
            now(),
        )
        .unwrap();
        assert!(r.component.is_none());
        assert!(r.to_markdown().contains("unspecified"));
    }
}
