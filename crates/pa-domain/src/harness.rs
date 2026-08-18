//! The continual harness: durable, supplemental state that a session accretes.
//!
//! Prime Agent's rule is reproduced exactly and is the reason this module has
//! teeth: **the base system prompt is immutable**. Refinement adds, updates and
//! removes supplemental entries and records a before/after snapshot for each
//! change, so a bad lesson can be undone rather than argued with.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// A supplemental instruction added to the startup prompt.
    PromptNote,
    /// A durable fact about the user, the project or the world.
    Memory,
    /// A description of a reusable call — the contract, not the package.
    SkillSpec,
    /// A delegation role worth reusing: prompt, model, tools.
    SubagentSpec,
}

impl EntryKind {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().replace(['-', ' '], "_").as_str() {
            "prompt_note" | "note" | "prompt" => Ok(Self::PromptNote),
            "memory" => Ok(Self::Memory),
            "skill_spec" | "skill" => Ok(Self::SkillSpec),
            "subagent_spec" | "subagent" | "agent" => Ok(Self::SubagentSpec),
            other => Err(DomainError::invalid(format!(
                "unknown harness kind '{other}'; expected prompt-note, memory, skill or subagent"
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::PromptNote => "prompt-note",
            Self::Memory => "memory",
            Self::SkillSpec => "skill",
            Self::SubagentSpec => "subagent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HarnessScope {
    /// Belongs to one session's artifact directory. The default: most lessons
    /// are about this repository, not about the user.
    Local,
    /// Belongs to the machine, under the global harness directory. Rare, and
    /// deliberately harder to reach for.
    Global,
}

impl HarnessScope {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "local" | "session" | "project" => Ok(Self::Local),
            "global" | "user" => Ok(Self::Global),
            other => Err(DomainError::invalid(format!(
                "unknown scope '{other}'; expected local or global"
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Global => "global",
        }
    }
}

/// One durable lesson.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HarnessEntry {
    pub id: String,
    pub kind: EntryKind,
    pub scope: HarnessScope,
    /// Short, stable, human-addressable. Unique within (kind, scope).
    pub name: String,
    pub body: String,
    /// What in the trajectory justified writing this down. Required: an entry
    /// nobody can justify later is an entry nobody can safely delete.
    pub evidence: String,
    /// What should improve, and how it would be checked.
    pub outcome: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// How many times this entry has been surfaced into a prompt. Cheap signal
    /// for pruning entries that never earn their tokens.
    #[serde(default)]
    pub hits: u64,
}

impl HarnessEntry {
    pub fn new(
        id: impl Into<String>,
        kind: EntryKind,
        scope: HarnessScope,
        name: &str,
        body: &str,
        evidence: &str,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let name = Self::normalise_name(name)?;
        let body = body.trim();
        if body.is_empty() {
            return Err(DomainError::invalid("a harness entry needs a body"));
        }
        let evidence = evidence.trim();
        if evidence.is_empty() {
            return Err(DomainError::invalid(
                "a harness entry needs evidence; say what in this session justified it",
            ));
        }
        Ok(Self {
            id: id.into(),
            kind,
            scope,
            name,
            body: body.to_string(),
            evidence: evidence.to_string(),
            outcome: None,
            tags: Vec::new(),
            created_at: now,
            updated_at: now,
            hits: 0,
        })
    }

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
            return Err(DomainError::invalid("a harness entry needs a name"));
        }
        if name.len() > 64 {
            return Err(DomainError::invalid("harness names are 64 characters or less"));
        }
        Ok(name)
    }

    /// The one-line form used when entries are listed into a prompt.
    pub fn headline(&self) -> String {
        let first = self.body.lines().next().unwrap_or("").trim();
        let clipped: String = if first.chars().count() > 96 {
            first.chars().take(93).chain("...".chars()).collect()
        } else {
            first.to_string()
        };
        format!("[{}] {}: {}", self.kind.label(), self.name, clipped)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefinementOp {
    Create,
    Update,
    Delete,
}

impl RefinementOp {
    pub fn label(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

/// The before/after pair that makes a refinement reversible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub before: Option<HarnessEntry>,
    pub after: Option<HarnessEntry>,
}

impl Snapshot {
    /// Applying a refinement backwards is just swapping the two halves.
    pub fn inverted(&self) -> Snapshot {
        Snapshot {
            before: self.after.clone(),
            after: self.before.clone(),
        }
    }
}

/// One recorded change to the harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refinement {
    pub id: String,
    pub session_id: Option<String>,
    pub op: RefinementOp,
    pub summary: String,
    pub evidence: String,
    pub outcome: Option<String>,
    pub snapshot: Snapshot,
    pub created_at: DateTime<Utc>,
    /// Set when this refinement has been rolled back, naming the refinement
    /// that undid it. Rollbacks are recorded, never erased.
    pub reverted_by: Option<String>,
}

impl Refinement {
    pub fn new(
        id: impl Into<String>,
        session_id: Option<String>,
        op: RefinementOp,
        summary: &str,
        evidence: &str,
        snapshot: Snapshot,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let summary = summary.trim();
        if summary.is_empty() {
            return Err(DomainError::invalid("a refinement needs a one-line summary"));
        }
        let evidence = evidence.trim();
        if evidence.is_empty() {
            return Err(DomainError::invalid(
                "a refinement needs evidence; an unjustified edit cannot be reviewed later",
            ));
        }
        match (op, &snapshot.before, &snapshot.after) {
            (RefinementOp::Create, Some(_), _) => {
                return Err(DomainError::invalid("a create refinement has no 'before'"))
            }
            (RefinementOp::Create, _, None) | (RefinementOp::Update, _, None) => {
                return Err(DomainError::invalid("this refinement needs an 'after'"))
            }
            (RefinementOp::Update, None, _) | (RefinementOp::Delete, None, _) => {
                return Err(DomainError::invalid("this refinement needs a 'before'"))
            }
            (RefinementOp::Delete, _, Some(_)) => {
                return Err(DomainError::invalid("a delete refinement has no 'after'"))
            }
            _ => {}
        }
        Ok(Self {
            id: id.into(),
            session_id,
            op,
            summary: summary.to_string(),
            evidence: evidence.to_string(),
            outcome: None,
            snapshot,
            created_at: now,
            reverted_by: None,
        })
    }

    pub fn is_reverted(&self) -> bool {
        self.reverted_by.is_some()
    }

    /// The operation that undoes this one.
    pub fn inverse_op(&self) -> RefinementOp {
        match self.op {
            RefinementOp::Create => RefinementOp::Delete,
            RefinementOp::Delete => RefinementOp::Create,
            RefinementOp::Update => RefinementOp::Update,
        }
    }
}

/// One harness change a review pass thinks is worth making.
///
/// # Why a proposal is not an entry
///
/// The discipline the harness enforces — every entry justified by evidence
/// somebody can weigh later — only works if a human, or at least a second
/// decision, stands between noticing something and writing it down. A review
/// that wrote directly to the harness would fill it with plausible-sounding
/// lessons drawn from single events, which is precisely the failure mode
/// `--evidence` exists to prevent.
///
/// So a review returns these, and applying them is a separate, explicit act.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub name: String,
    pub kind: EntryKind,
    pub scope: HarnessScope,
    pub body: String,
    pub evidence: String,
    pub outcome: Option<String>,
}

impl Proposal {
    /// Reads proposals out of a model's reply.
    ///
    /// Tolerant of the prose harnesses like to wrap JSON in — the first `[`
    /// through the last `]` is taken as the payload — because failing the
    /// whole review over a "Here's what I found:" preamble would make this
    /// unusable against real runners.
    ///
    /// Malformed *entries* are a different matter from malformed output: a
    /// proposal with no evidence is dropped rather than repaired, since
    /// inventing the justification is exactly the thing that must not happen.
    pub fn parse_many(reply: &str) -> Result<Vec<Self>> {
        let start = reply.find('[');
        let end = reply.rfind(']');
        let payload = match (start, end) {
            (Some(start), Some(end)) if end > start => &reply[start..=end],
            // No array at all is a legitimate answer: nothing was worth
            // remembering. It is not an error, and it is the common case.
            _ => return Ok(Vec::new()),
        };

        let rows: Vec<RawProposal> = serde_json::from_str(payload)
            .map_err(|error| DomainError::invalid(format!("unreadable review: {error}")))?;

        Ok(rows.into_iter().filter_map(RawProposal::into_proposal).collect())
    }
}

/// The wire shape, deliberately forgiving where the strict type is not: a
/// model that writes `"kind": "note"` instead of `"prompt-note"` should lose
/// that one proposal, not the whole review.
#[derive(Debug, Deserialize)]
struct RawProposal {
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    body: String,
    #[serde(default)]
    evidence: Option<String>,
    #[serde(default)]
    outcome: Option<String>,
}

impl RawProposal {
    fn into_proposal(self) -> Option<Proposal> {
        let name = HarnessEntry::normalise_name(&self.name).ok()?;
        let body = self.body.trim().to_string();
        let evidence = self.evidence.unwrap_or_default().trim().to_string();
        // The one rule that is not negotiable anywhere in this crate.
        if body.is_empty() || evidence.is_empty() {
            return None;
        }
        Some(Proposal {
            name,
            kind: self
                .kind
                .as_deref()
                .and_then(|k| EntryKind::parse(k).ok())
                .unwrap_or(EntryKind::Memory),
            scope: self
                .scope
                .as_deref()
                .and_then(|s| HarnessScope::parse(s).ok())
                .unwrap_or(HarnessScope::Local),
            body,
            evidence,
            outcome: self.outcome.map(|o| o.trim().to_string()).filter(|o| !o.is_empty()),
        })
    }
}


#[cfg(test)]
mod proposal_tests {
    use super::*;

    #[test]
    fn json_is_found_inside_whatever_prose_the_model_wrapped_it_in() {
        let reply = r#"Here is what I noticed:
        [{"name": "prefers-rust", "kind": "memory", "body": "Wants memory-safe languages.",
          "evidence": "Asked for Rust twice."}]
        Let me know if you want these applied."#;

        let proposals = Proposal::parse_many(reply).unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].name, "prefers-rust");
    }

    #[test]
    fn a_proposal_with_no_evidence_is_dropped_not_repaired() {
        // The whole discipline of this module. A review that invented the
        // justification would defeat the reason `--evidence` is required.
        let reply = r#"[
          {"name": "unjustified", "body": "They like tabs."},
          {"name": "justified", "body": "They like tabs.", "evidence": "Said so, twice."}
        ]"#;

        let proposals = Proposal::parse_many(reply).unwrap();
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].name, "justified");
    }

    #[test]
    fn one_unreadable_field_costs_that_proposal_its_kind_not_the_review() {
        let reply = r#"[{"name": "n", "kind": "nonsense", "scope": "nonsense",
                         "body": "b", "evidence": "e"}]"#;

        let proposals = Proposal::parse_many(reply).unwrap();
        // Falls back rather than failing: the lesson is worth more than the
        // label, and a wrong label is one `pa harness` call to fix.
        assert_eq!(proposals[0].kind, EntryKind::Memory);
        assert_eq!(proposals[0].scope, HarnessScope::Local);
    }

    #[test]
    fn nothing_worth_remembering_is_an_answer_rather_than_an_error() {
        assert!(Proposal::parse_many("I found nothing worth keeping.")
            .unwrap()
            .is_empty());
        assert!(Proposal::parse_many("[]").unwrap().is_empty());
    }

    #[test]
    fn genuinely_broken_json_is_reported() {
        // An unterminated array is indistinguishable from prose with a stray
        // bracket, so it reads as "nothing proposed" — but a complete array
        // whose contents are malformed is a real failure worth surfacing.
        assert!(Proposal::parse_many(r#"[{"name": }]"#).is_err());
        assert!(Proposal::parse_many(r#"[{"name": "n""#).unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn entry() -> HarnessEntry {
        HarnessEntry::new(
            "ent-1",
            EntryKind::Memory,
            HarnessScope::Local,
            "Prefers Rust",
            "The user asked for a memory-safe language.",
            "Said so explicitly in the session.",
            now(),
        )
        .unwrap()
    }

    #[test]
    fn entries_demand_evidence() {
        let bad = HarnessEntry::new(
            "ent-1",
            EntryKind::Memory,
            HarnessScope::Local,
            "thing",
            "body",
            "  ",
            now(),
        );
        assert!(bad.is_err());
    }

    #[test]
    fn names_are_normalised() {
        assert_eq!(entry().name, "prefers-rust");
    }

    #[test]
    fn refinements_must_be_shaped_like_their_operation() {
        let full = Snapshot {
            before: Some(entry()),
            after: Some(entry()),
        };
        assert!(Refinement::new(
            "ref-1",
            None,
            RefinementOp::Create,
            "add",
            "because",
            full.clone(),
            now()
        )
        .is_err());

        assert!(Refinement::new(
            "ref-1",
            None,
            RefinementOp::Delete,
            "remove",
            "because",
            full,
            now()
        )
        .is_err());

        assert!(Refinement::new(
            "ref-1",
            None,
            RefinementOp::Create,
            "add",
            "because",
            Snapshot {
                before: None,
                after: Some(entry())
            },
            now()
        )
        .is_ok());
    }

    #[test]
    fn a_snapshot_inverts_cleanly() {
        let snapshot = Snapshot {
            before: None,
            after: Some(entry()),
        };
        let back = snapshot.inverted();
        assert!(back.after.is_none());
        assert!(back.before.is_some());
    }
}
