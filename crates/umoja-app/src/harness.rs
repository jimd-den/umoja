//! The continual harness and `/refine`.
//!
//! Two rules run through everything here:
//!
//! 1. **Nothing is written without evidence.** The domain refuses an entry or a
//!    refinement with an empty justification, and this service never invents
//!    one on the caller's behalf.
//! 2. **Every write is reversible.** Each change records a before/after
//!    snapshot, so [`HarnessService::rollback`] is a mechanical inverse rather
//!    than a second guess at what the state used to be.

use std::sync::Arc;

use umoja_domain::prelude::*;
use umoja_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

#[derive(Debug, Clone)]
pub struct Remember {
    pub session_id: Option<String>,
    pub kind: EntryKind,
    pub scope: HarnessScope,
    pub name: String,
    pub body: String,
    pub evidence: String,
    pub outcome: Option<String>,
    pub tags: Vec<String>,
}

pub struct HarnessService {
    env: Env,
    store: Arc<dyn HarnessStore>,
    transcript: Arc<dyn TranscriptLog>,
}

impl std::fmt::Debug for HarnessService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HarnessService")
    }
}

impl HarnessService {
    pub fn new(env: Env, store: Arc<dyn HarnessStore>, transcript: Arc<dyn TranscriptLog>) -> Self {
        Self {
            env,
            store,
            transcript,
        }
    }

    /// Creates or updates an entry, recording the change either way.
    pub fn remember(&self, request: Remember) -> Result<(HarnessEntry, Refinement)> {
        let now = self.env.now();
        let session = request.session_id.as_deref();
        let name = HarnessEntry::normalise_name(&request.name)?;

        let existing = self.store.get(session, request.scope, &name).ok();

        let mut entry = HarnessEntry::new(
            existing
                .as_ref()
                .map(|prior| prior.id.clone())
                .unwrap_or_else(|| self.env.id(Ids::ENTRY)),
            request.kind,
            request.scope,
            &name,
            &request.body,
            &request.evidence,
            now,
        )?;
        entry.outcome = request.outcome;
        entry.tags = request.tags;
        if let Some(prior) = &existing {
            // Age and usefulness belong to the entry, not to this edit.
            entry.created_at = prior.created_at;
            entry.hits = prior.hits;
        }

        let op = if existing.is_some() {
            RefinementOp::Update
        } else {
            RefinementOp::Create
        };

        let refinement = Refinement::new(
            self.env.id(Ids::REFINEMENT),
            request.session_id.clone(),
            op,
            &format!("{} {} '{}'", op.label(), request.kind.label(), name),
            &entry.evidence,
            Snapshot {
                before: existing,
                after: Some(entry.clone()),
            },
            now,
        )?;

        self.store.upsert(session, &entry)?;
        self.store.record_refinement(session, &refinement)?;
        self.log(session, &refinement, now)?;

        Ok((entry, refinement))
    }

    /// Removes an entry, keeping enough of it to put it back.
    pub fn forget(
        &self,
        session_id: Option<&str>,
        scope: HarnessScope,
        name: &str,
        evidence: &str,
    ) -> Result<Refinement> {
        let now = self.env.now();
        let name = HarnessEntry::normalise_name(name)?;
        let existing = self.store.get(session_id, scope, &name)?;

        let refinement = Refinement::new(
            self.env.id(Ids::REFINEMENT),
            session_id.map(str::to_string),
            RefinementOp::Delete,
            &format!("delete {} '{}'", existing.kind.label(), name),
            evidence,
            Snapshot {
                before: Some(existing),
                after: None,
            },
            now,
        )?;

        self.store.remove(session_id, scope, &name)?;
        self.store.record_refinement(session_id, &refinement)?;
        self.log(session_id, &refinement, now)?;

        Ok(refinement)
    }

    /// Undoes a refinement by applying its snapshot backwards.
    ///
    /// The rollback is itself recorded, and the original is stamped with the id
    /// that reverted it. Nothing is erased: the history of what was tried is
    /// the only reason any of this is trustworthy.
    pub fn rollback(&self, session_id: Option<&str>, refinement_id: &str) -> Result<Refinement> {
        let now = self.env.now();
        let mut original = self.store.refinement(session_id, refinement_id)?;
        if original.is_reverted() {
            return Err(DomainError::forbidden(format!(
                "{refinement_id} was already rolled back by {}",
                original.reverted_by.clone().unwrap_or_default()
            )));
        }

        let inverse = original.snapshot.inverted();
        match (&inverse.before, &inverse.after) {
            // Undoing a create: remove what it added.
            (Some(prior), None) => self.store.remove(session_id, prior.scope, &prior.name)?,
            // Undoing a delete or an update: put the old version back.
            (_, Some(restored)) => self.store.upsert(session_id, restored)?,
            (None, None) => {
                return Err(DomainError::invalid("that refinement has nothing to undo"))
            }
        }

        let undo = Refinement::new(
            self.env.id(Ids::REFINEMENT),
            session_id.map(str::to_string),
            original.inverse_op(),
            &format!("rollback of {refinement_id}: {}", original.summary),
            &format!("undoing refinement {refinement_id}"),
            inverse,
            now,
        )?;

        original.reverted_by = Some(undo.id.clone());
        self.store.update_refinement(session_id, &original)?;
        self.store.record_refinement(session_id, &undo)?;
        self.log(session_id, &undo, now)?;

        Ok(undo)
    }

    pub fn list(
        &self,
        session_id: Option<&str>,
        kind: Option<EntryKind>,
    ) -> Result<Vec<HarnessEntry>> {
        let mut rows = self.store.list(session_id)?;
        rows.retain(|row| kind.is_none_or(|wanted| row.kind == wanted));
        rows.sort_by(|a, b| {
            a.kind
                .label()
                .cmp(b.kind.label())
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(rows)
    }

    pub fn refinements(
        &self,
        session_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Refinement>> {
        self.store.refinements(session_id, limit)
    }

    /// Renders the harness as a prompt block.
    ///
    /// Headlines only, grouped by kind. The base prompt is never touched; this
    /// is supplemental text a caller appends, which is exactly the boundary
    /// prime-agent draws.
    pub fn prompt_block(&self, session_id: Option<&str>) -> Result<String> {
        let entries = self.list(session_id, None)?;
        if entries.is_empty() {
            return Ok(String::new());
        }

        let mut out = String::from("<harness>\n");
        for kind in [
            EntryKind::PromptNote,
            EntryKind::Memory,
            EntryKind::SkillSpec,
            EntryKind::SubagentSpec,
        ] {
            let group: Vec<&HarnessEntry> =
                entries.iter().filter(|row| row.kind == kind).collect();
            if group.is_empty() {
                continue;
            }
            out.push_str(&format!("  <{}>\n", kind.label()));
            for entry in group {
                out.push_str(&format!("    {}\n", entry.headline()));
            }
            out.push_str(&format!("  </{}>\n", kind.label()));
        }
        out.push_str("</harness>");
        Ok(out)
    }

    fn log(
        &self,
        session_id: Option<&str>,
        refinement: &Refinement,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        let Some(session_id) = session_id else {
            return Ok(());
        };
        self.transcript.append(&TranscriptRecord::new(
            session_id,
            now,
            TranscriptEvent::Refined {
                refinement_id: refinement.id.clone(),
                op: refinement.op.label().to_string(),
                summary: refinement.summary.clone(),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;

    fn service() -> HarnessService {
        let (env, _clock) = env();
        HarnessService::new(
            env,
            Arc::new(MemHarness::default()),
            Arc::new(MemTranscript::default()),
        )
    }

    fn remember(name: &str, body: &str) -> Remember {
        Remember {
            session_id: Some("ses-1".into()),
            kind: EntryKind::Memory,
            scope: HarnessScope::Local,
            name: name.into(),
            body: body.into(),
            evidence: "the user said so".into(),
            outcome: None,
            tags: vec![],
        }
    }

    #[test]
    fn writing_twice_updates_rather_than_duplicates() {
        let service = service();
        let (_, first) = service.remember(remember("style", "prefers rust")).unwrap();
        assert_eq!(first.op, RefinementOp::Create);

        let (entry, second) = service
            .remember(remember("style", "prefers rust, no unsafe"))
            .unwrap();
        assert_eq!(second.op, RefinementOp::Update);
        assert_eq!(entry.body, "prefers rust, no unsafe");
        assert_eq!(service.list(Some("ses-1"), None).unwrap().len(), 1);
    }

    #[test]
    fn an_entry_without_evidence_is_refused() {
        let service = service();
        let mut request = remember("style", "body");
        request.evidence = "   ".into();
        assert!(service.remember(request).is_err());
    }

    #[test]
    fn rolling_back_a_create_removes_the_entry() {
        let service = service();
        let (_, refinement) = service.remember(remember("style", "prefers rust")).unwrap();
        service.rollback(Some("ses-1"), &refinement.id).unwrap();
        assert!(service.list(Some("ses-1"), None).unwrap().is_empty());
    }

    #[test]
    fn rolling_back_an_update_restores_the_previous_body() {
        let service = service();
        service.remember(remember("style", "first")).unwrap();
        let (_, update) = service.remember(remember("style", "second")).unwrap();
        service.rollback(Some("ses-1"), &update.id).unwrap();

        let rows = service.list(Some("ses-1"), None).unwrap();
        assert_eq!(rows[0].body, "first");
    }

    #[test]
    fn rolling_back_a_delete_puts_the_entry_back() {
        let service = service();
        service.remember(remember("style", "prefers rust")).unwrap();
        let deletion = service
            .forget(Some("ses-1"), HarnessScope::Local, "style", "no longer true")
            .unwrap();
        assert!(service.list(Some("ses-1"), None).unwrap().is_empty());

        service.rollback(Some("ses-1"), &deletion.id).unwrap();
        assert_eq!(service.list(Some("ses-1"), None).unwrap()[0].body, "prefers rust");
    }

    #[test]
    fn a_rollback_cannot_be_applied_twice() {
        let service = service();
        let (_, refinement) = service.remember(remember("style", "prefers rust")).unwrap();
        service.rollback(Some("ses-1"), &refinement.id).unwrap();
        assert!(service.rollback(Some("ses-1"), &refinement.id).is_err());
    }

    #[test]
    fn global_entries_are_visible_from_any_session() {
        let service = service();
        let mut global = remember("tone", "terse");
        global.scope = HarnessScope::Global;
        service.remember(global).unwrap();
        service.remember(remember("local", "this repo")).unwrap();

        assert_eq!(service.list(Some("ses-1"), None).unwrap().len(), 2);
        assert_eq!(service.list(Some("ses-other"), None).unwrap().len(), 1);
    }

    #[test]
    fn the_prompt_block_carries_headlines_not_bodies() {
        let service = service();
        service
            .remember(remember(
                "style",
                "prefers rust\nand a great deal of further detail that must not appear",
            ))
            .unwrap();
        let block = service.prompt_block(Some("ses-1")).unwrap();
        assert!(block.contains("prefers rust"));
        assert!(!block.contains("must not appear"));
        assert!(block.starts_with("<harness>"));
    }

    #[test]
    fn an_empty_harness_renders_nothing_at_all() {
        assert!(service().prompt_block(Some("ses-1")).unwrap().is_empty());
    }
}
