//! Filesystem implementations of the storage ports.
//!
//! Each one is a thin translation between a domain port and a [`JsonTable`] or
//! JSONL file. The rules live in `pa-app` and `pa-domain`; nothing here decides
//! anything except where bytes go.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use umoja_domain::prelude::*;
use umoja_domain::transcript::TranscriptRecord;

use crate::files::{append_jsonl, read_jsonl};
use crate::paths::Paths;
use crate::table::JsonTable;

/// Registries that only grow are trimmed to this many rows on write.
const MESSAGE_CEILING: usize = 2_000;

pub struct FsSessionStore {
    table: JsonTable<Session>,
}

impl std::fmt::Debug for FsSessionStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FsSessionStore")
    }
}

impl FsSessionStore {
    pub fn new(paths: &Paths) -> Self {
        Self {
            table: JsonTable::new(paths.registry("sessions")),
        }
    }
}

impl SessionStore for FsSessionStore {
    fn insert(&self, session: &Session) -> Result<()> {
        self.table.mutate(|rows| {
            if rows.iter().any(|row| row.id == session.id) {
                return Err(DomainError::conflict("session", &session.id));
            }
            if rows.iter().any(|row| row.name == session.name) {
                return Err(DomainError::conflict("session name", &session.name));
            }
            rows.push(session.clone());
            Ok(())
        })
    }

    fn update(&self, session: &Session) -> Result<()> {
        self.table.mutate(|rows| {
            let slot = rows
                .iter_mut()
                .find(|row| row.id == session.id)
                .ok_or_else(|| DomainError::not_found("session", &session.id))?;
            *slot = session.clone();
            Ok(())
        })
    }

    fn get(&self, id: &str) -> Result<Session> {
        self.table
            .find(|row| row.id == id)?
            .ok_or_else(|| DomainError::not_found("session", id))
    }

    fn resolve(&self, selector: &str) -> Result<Session> {
        let rows = self.table.rows()?;
        let hits: Vec<Session> = rows
            .into_iter()
            .filter(|row| row.matches_selector(selector))
            .collect();
        match hits.len() {
            0 => Err(DomainError::not_found("session", selector)),
            1 => Ok(hits.into_iter().next().expect("checked length")),
            _ => Err(DomainError::conflict("session selector", selector)),
        }
    }

    fn list(&self) -> Result<Vec<Session>> {
        self.table.rows()
    }

    fn children_of(&self, parent_id: &str) -> Result<Vec<Session>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| row.parent_id.as_deref() == Some(parent_id))
            .collect())
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.table.remove(|row| row.id == id)?;
        Ok(())
    }
}

/// One JSONL file per session, appended to and never rewritten.
pub struct FsTranscriptLog {
    paths: Paths,
}

impl std::fmt::Debug for FsTranscriptLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FsTranscriptLog")
    }
}

impl FsTranscriptLog {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }
}

impl TranscriptLog for FsTranscriptLog {
    fn append(&self, record: &TranscriptRecord) -> Result<()> {
        append_jsonl(&self.paths.transcript(&record.session_id), record)
    }

    fn read(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<TranscriptRecord>> {
        read_jsonl(&self.paths.transcript(session_id), limit)
    }
}

/// Local entries live beside the session; global ones live under the home.
pub struct FsHarnessStore {
    paths: Paths,
}

impl std::fmt::Debug for FsHarnessStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FsHarnessStore")
    }
}

impl FsHarnessStore {
    pub fn new(paths: Paths) -> Self {
        Self { paths }
    }

    fn entries_at(&self, session_id: Option<&str>, scope: HarnessScope) -> PathBuf {
        match (scope, session_id) {
            (HarnessScope::Global, _) => self.paths.global_harness(),
            (HarnessScope::Local, Some(session)) => self.paths.local_harness(session),
            // A local write with no session has nowhere session-shaped to go,
            // so it lands in the global file rather than being dropped.
            (HarnessScope::Local, None) => self.paths.global_harness(),
        }
    }

    fn table(&self, session_id: Option<&str>, scope: HarnessScope) -> JsonTable<HarnessEntry> {
        JsonTable::new(self.entries_at(session_id, scope))
    }

    fn refinement_log(&self, session_id: Option<&str>) -> PathBuf {
        match session_id {
            Some(session) => self.paths.local_refinements(session),
            None => self.paths.global_refinements(),
        }
    }
}

impl HarnessStore for FsHarnessStore {
    fn upsert(&self, session_id: Option<&str>, entry: &HarnessEntry) -> Result<()> {
        self.table(session_id, entry.scope)
            .upsert(entry, |row| row.name == entry.name && row.scope == entry.scope)
    }

    fn get(
        &self,
        session_id: Option<&str>,
        scope: HarnessScope,
        name: &str,
    ) -> Result<HarnessEntry> {
        self.table(session_id, scope)
            .find(|row| row.name == name && row.scope == scope)?
            .ok_or_else(|| DomainError::not_found("harness entry", name))
    }

    fn remove(&self, session_id: Option<&str>, scope: HarnessScope, name: &str) -> Result<()> {
        let removed = self
            .table(session_id, scope)
            .remove(|row| row.name == name && row.scope == scope)?;
        if removed == 0 {
            return Err(DomainError::not_found("harness entry", name));
        }
        Ok(())
    }

    fn list(&self, session_id: Option<&str>) -> Result<Vec<HarnessEntry>> {
        let mut rows = self.table(None, HarnessScope::Global).rows()?;
        if let Some(session) = session_id {
            rows.extend(
                self.table(Some(session), HarnessScope::Local)
                    .rows()?
                    .into_iter()
                    .filter(|row| row.scope == HarnessScope::Local),
            );
        }
        rows.retain(|row| row.scope == HarnessScope::Global || session_id.is_some());
        Ok(rows)
    }

    fn record_refinement(&self, session_id: Option<&str>, refinement: &Refinement) -> Result<()> {
        append_jsonl(&self.refinement_log(session_id), refinement)
    }

    /// Updating a refinement means appending its new version.
    ///
    /// The log is append-only, so "reverted" is recorded as a later line rather
    /// than by editing history. [`Self::refinements`] keeps the last version of
    /// each id, which is what a reader wants to see.
    fn update_refinement(&self, session_id: Option<&str>, refinement: &Refinement) -> Result<()> {
        append_jsonl(&self.refinement_log(session_id), refinement)
    }

    fn refinements(
        &self,
        session_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Refinement>> {
        let rows: Vec<Refinement> = read_jsonl(&self.refinement_log(session_id), None)?;

        let mut latest: Vec<Refinement> = Vec::new();
        for row in rows {
            match latest.iter_mut().find(|kept| kept.id == row.id) {
                Some(slot) => *slot = row,
                None => latest.push(row),
            }
        }

        latest.reverse();
        if let Some(limit) = limit {
            latest.truncate(limit);
        }
        Ok(latest)
    }

    fn refinement(&self, session_id: Option<&str>, id: &str) -> Result<Refinement> {
        self.refinements(session_id, None)?
            .into_iter()
            .find(|row| row.id == id)
            .ok_or_else(|| DomainError::not_found("refinement", id))
    }
}

macro_rules! json_registry {
    ($name:ident, $row:ty, $file:literal) => {
        pub struct $name {
            table: JsonTable<$row>,
        }

        impl std::fmt::Debug for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(stringify!($name))
            }
        }

        impl $name {
            pub fn new(paths: &Paths) -> Self {
                Self {
                    table: JsonTable::new(paths.registry($file)),
                }
            }
        }
    };
}

json_registry!(FsGoalStore, Goal, "goals");
json_registry!(FsHeartbeatStore, Heartbeat, "heartbeats");
json_registry!(FsScheduleStore, ScheduledJob, "schedules");
json_registry!(FsMessageStore, AgentMessage, "messages");
json_registry!(FsSubagentRegistry, Subagent, "subagents");
json_registry!(FsAutonomousStore, AutonomousState, "autonomous");
json_registry!(FsCompactionStore, CompactionState, "compaction");

impl GoalStore for FsGoalStore {
    fn put(&self, goal: &Goal) -> Result<()> {
        self.table
            .upsert(goal, |row| row.session_id == goal.session_id)
    }

    fn get(&self, session_id: &str) -> Result<Option<Goal>> {
        self.table.find(|row| row.session_id == session_id)
    }

    fn clear(&self, session_id: &str) -> Result<()> {
        self.table.remove(|row| row.session_id == session_id)?;
        Ok(())
    }

    fn active(&self) -> Result<Vec<Goal>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| row.should_continue())
            .collect())
    }
}

impl HeartbeatStore for FsHeartbeatStore {
    fn put(&self, heartbeat: &Heartbeat) -> Result<()> {
        self.table.upsert(heartbeat, |row| row.id == heartbeat.id)
    }

    fn get(&self, id: &str) -> Result<Heartbeat> {
        self.table
            .find(|row| row.id == id)?
            .ok_or_else(|| DomainError::not_found("heartbeat", id))
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.table.remove(|row| row.id == id)?;
        Ok(())
    }

    fn list(&self, session_id: Option<&str>) -> Result<Vec<Heartbeat>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| session_id.is_none_or(|id| row.session_id == id))
            .collect())
    }

    fn due(&self, now: DateTime<Utc>) -> Result<Vec<Heartbeat>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| row.is_due(now))
            .collect())
    }
}

impl ScheduleStore for FsScheduleStore {
    fn put(&self, job: &ScheduledJob) -> Result<()> {
        self.table.upsert(job, |row| row.id == job.id)
    }

    fn get(&self, id: &str) -> Result<ScheduledJob> {
        self.table
            .find(|row| row.id == id)?
            .ok_or_else(|| DomainError::not_found("job", id))
    }

    fn list(&self, target: Option<&str>, include_finished: bool) -> Result<Vec<ScheduledJob>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| target.is_none_or(|name| row.target == name))
            .filter(|row| {
                include_finished || matches!(row.status, JobStatus::Pending | JobStatus::Claimed)
            })
            .collect())
    }

    fn due(&self, now: DateTime<Utc>) -> Result<Vec<ScheduledJob>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| row.is_due(now))
            .collect())
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.table.remove(|row| row.id == id)?;
        Ok(())
    }
}

impl MessageStore for FsMessageStore {
    fn enqueue(&self, message: &AgentMessage) -> Result<()> {
        self.table.mutate(|rows| {
            rows.push(message.clone());
            Ok(())
        })?;
        self.table.trim_to(MESSAGE_CEILING)?;
        Ok(())
    }

    fn update(&self, message: &AgentMessage) -> Result<()> {
        self.table.mutate(|rows| {
            let slot = rows
                .iter_mut()
                .find(|row| row.id == message.id)
                .ok_or_else(|| DomainError::not_found("message", &message.id))?;
            *slot = message.clone();
            Ok(())
        })
    }

    fn pending_for(&self, session_id: &str) -> Result<Vec<AgentMessage>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| row.receiver_session_id == session_id && row.is_pending())
            .collect())
    }

    fn outbox(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<AgentMessage>> {
        let mut rows: Vec<AgentMessage> = self
            .table
            .rows()?
            .into_iter()
            .filter(|row| row.sender_session_id == session_id)
            .collect();
        rows.reverse();
        if let Some(limit) = limit {
            rows.truncate(limit);
        }
        Ok(rows)
    }

    fn get(&self, id: &str) -> Result<AgentMessage> {
        self.table
            .find(|row| row.id == id)?
            .ok_or_else(|| DomainError::not_found("message", id))
    }
}

impl SubagentRegistry for FsSubagentRegistry {
    fn insert(&self, child: &Subagent) -> Result<()> {
        self.table.mutate(|rows| {
            if rows.iter().any(|row| {
                row.parent_session_id == child.parent_session_id && row.name == child.name
            }) {
                return Err(DomainError::conflict("subagent name", &child.name));
            }
            rows.push(child.clone());
            Ok(())
        })
    }

    fn update(&self, child: &Subagent) -> Result<()> {
        self.table.mutate(|rows| {
            let slot = rows
                .iter_mut()
                .find(|row| row.child_id == child.child_id)
                .ok_or_else(|| DomainError::not_found("subagent", &child.child_id))?;
            *slot = child.clone();
            Ok(())
        })
    }

    fn get(&self, parent_session_id: &str, selector: &str) -> Result<Subagent> {
        self.table
            .find(|row| {
                row.parent_session_id == parent_session_id && row.matches_selector(selector)
            })?
            .ok_or_else(|| DomainError::not_found("subagent", selector))
    }

    fn list(&self, parent_session_id: &str, include_deleted: bool) -> Result<Vec<Subagent>> {
        Ok(self
            .table
            .rows()?
            .into_iter()
            .filter(|row| row.parent_session_id == parent_session_id)
            .filter(|row| include_deleted || row.status != SubagentStatus::Deleted)
            .collect())
    }

    fn all(&self) -> Result<Vec<Subagent>> {
        self.table.rows()
    }
}

impl AutonomousStore for FsAutonomousStore {
    fn put(&self, state: &AutonomousState) -> Result<()> {
        self.table
            .upsert(state, |row| row.session_id == state.session_id)
    }

    fn get(&self, session_id: &str) -> Result<Option<AutonomousState>> {
        self.table.find(|row| row.session_id == session_id)
    }

    fn clear(&self, session_id: &str) -> Result<()> {
        self.table.remove(|row| row.session_id == session_id)?;
        Ok(())
    }
}

impl CompactionStore for FsCompactionStore {
    fn put(&self, state: &CompactionState) -> Result<()> {
        self.table
            .upsert(state, |row| row.session_id == state.session_id)
    }

    fn get(&self, session_id: &str) -> Result<Option<CompactionState>> {
        self.table.find(|row| row.session_id == session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(name: &str) -> Paths {
        let dir = std::env::temp_dir().join(format!("pa-stores-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Paths::at(dir).unwrap()
    }

    fn session(id: &str, name: &str) -> Session {
        Session {
            id: id.into(),
            name: name.into(),
            kind: SessionKind::Root,
            status: SessionStatus::Idle,
            workdir: "/work".into(),
            runner: "claude".into(),
            model: None,
            parent_id: None,
            depth: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            usage: Usage::default(),
            pid: None,
        }
    }

    #[test]
    fn sessions_survive_a_new_store_instance() {
        let paths = paths("sessions");
        FsSessionStore::new(&paths)
            .insert(&session("ses-1", "worker"))
            .unwrap();

        // A fresh instance reads the same file: this is what makes the CLI
        // one-shot yet stateful.
        let reopened = FsSessionStore::new(&paths);
        assert_eq!(reopened.resolve("worker").unwrap().id, "ses-1");
    }

    #[test]
    fn duplicate_names_are_refused_on_disk_too() {
        let paths = paths("dupes");
        let store = FsSessionStore::new(&paths);
        store.insert(&session("ses-1", "worker")).unwrap();
        assert!(store.insert(&session("ses-2", "worker")).is_err());
    }

    #[test]
    fn transcripts_append_and_read_back() {
        let paths = paths("transcript");
        let log = FsTranscriptLog::new(paths);
        for index in 0..5 {
            log.append(&TranscriptRecord::new(
                "ses-1",
                Utc::now(),
                umoja_domain::transcript::TranscriptEvent::UserPrompt {
                    text: format!("line {index}"),
                },
            ))
            .unwrap();
        }
        assert_eq!(log.read("ses-1", None).unwrap().len(), 5);
        assert_eq!(log.read("ses-1", Some(2)).unwrap().len(), 2);
        assert!(log.read("ses-unknown", None).unwrap().is_empty());
    }

    #[test]
    fn harness_scopes_are_stored_apart() {
        let paths = paths("harness");
        let store = FsHarnessStore::new(paths);
        let now = Utc::now();

        let local = HarnessEntry::new(
            "ent-1",
            EntryKind::Memory,
            HarnessScope::Local,
            "repo-fact",
            "this repo uses rust",
            "observed",
            now,
        )
        .unwrap();
        let global = HarnessEntry::new(
            "ent-2",
            EntryKind::Memory,
            HarnessScope::Global,
            "user-fact",
            "prefers terse output",
            "said so",
            now,
        )
        .unwrap();

        store.upsert(Some("ses-1"), &local).unwrap();
        store.upsert(Some("ses-1"), &global).unwrap();

        assert_eq!(store.list(Some("ses-1")).unwrap().len(), 2);
        // A different session sees the global entry only.
        assert_eq!(store.list(Some("ses-2")).unwrap().len(), 1);
    }

    #[test]
    fn a_refinement_update_is_appended_and_the_latest_version_wins() {
        let paths = paths("refinements");
        let store = FsHarnessStore::new(paths);
        let now = Utc::now();
        let entry = HarnessEntry::new(
            "ent-1",
            EntryKind::Memory,
            HarnessScope::Local,
            "fact",
            "body",
            "evidence",
            now,
        )
        .unwrap();

        let mut refinement = Refinement::new(
            "ref-1",
            Some("ses-1".into()),
            RefinementOp::Create,
            "add fact",
            "evidence",
            Snapshot {
                before: None,
                after: Some(entry),
            },
            now,
        )
        .unwrap();

        store.record_refinement(Some("ses-1"), &refinement).unwrap();
        refinement.reverted_by = Some("ref-2".into());
        store.update_refinement(Some("ses-1"), &refinement).unwrap();

        let rows = store.refinements(Some("ses-1"), None).unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_reverted());
    }
}
