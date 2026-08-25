//! SQLite adapter implementations for domain storage ports.

use chrono::{DateTime, Utc};
use rusqlite::params;
use umoja_domain::autonomous::AutonomousState;
use umoja_domain::compaction::CompactionState;
use umoja_domain::error::{DomainError, Result};
use umoja_domain::goal::{Goal, GoalStatus};
use umoja_domain::harness::{HarnessEntry, HarnessScope, Refinement};
use umoja_domain::heartbeat::Heartbeat;
use umoja_domain::message::AgentMessage;
use umoja_domain::ports::{
    AutonomousStore, CompactionStore, GoalStore, HarnessStore, HeartbeatStore, LineageStore,
    MessageStore, ScheduleStore, SessionStore, SubagentRegistry, TranscriptLog,
};
use umoja_domain::schedule::{JobStatus, ScheduledJob};
use umoja_domain::session::Session;
use umoja_domain::subagent::{Subagent, SubagentStatus};
use umoja_domain::transcript::TranscriptRecord;

use super::db::SqliteDb;

// -----------------------------------------------------------------------------
// SqliteSessionStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteSessionStore {
    db: SqliteDb,
}

impl SqliteSessionStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl SessionStore for SqliteSessionStore {
    fn insert(&self, session: &Session) -> Result<()> {
        let json = serde_json::to_string(session)
            .map_err(|e| DomainError::adapter("serialize session", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO sessions (id, name, created_at, updated_at, workdir, runner, status, data) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )?;
            stmt.execute(params![
                session.id,
                session.name,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                session.workdir,
                session.runner,
                serde_json::to_string(&session.status).unwrap_or_default(),
                json
            ])?;
            Ok(())
        })
    }

    fn update(&self, session: &Session) -> Result<()> {
        let json = serde_json::to_string(session)
            .map_err(|e| DomainError::adapter("serialize session", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "UPDATE sessions SET name = ?, updated_at = ?, workdir = ?, runner = ?, status = ?, data = ? WHERE id = ?"
            )?;
            let rows = stmt.execute(params![
                session.name,
                session.updated_at.to_rfc3339(),
                session.workdir,
                session.runner,
                serde_json::to_string(&session.status).unwrap_or_default(),
                json,
                session.id
            ])?;
            if rows == 0 {
                return Err(rusqlite::Error::ToSqlConversionFailure(Box::new(DomainError::not_found("session", &session.id))));
            }
            Ok(())
        })
    }

    fn get(&self, id: &str) -> Result<Session> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM sessions WHERE id = ?")?;
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let session: Session = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(session)
            } else {
                Err(rusqlite::Error::ToSqlConversionFailure(Box::new(DomainError::not_found("session", id))))
            }
        })
    }

    fn resolve(&self, selector: &str) -> Result<Session> {
        let sessions = self.list()?;
        let exact: Vec<&Session> = sessions.iter().filter(|s| s.matches_selector(selector)).collect();
        match exact.len() {
            0 => Err(DomainError::not_found("session", selector)),
            1 => Ok((*exact[0]).clone()),
            _ => Err(DomainError::conflict("session selector", selector)),
        }
    }

    fn list(&self) -> Result<Vec<Session>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM sessions ORDER BY created_at DESC")?;
            let sessions = stmt.query_map([], |row| {
                let data: String = row.get(0)?;
                let s: Session = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(s)
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(sessions)
        })
    }

    fn children_of(&self, parent_id: &str) -> Result<Vec<Session>> {
        let all = self.list()?;
        Ok(all.into_iter().filter(|s| s.parent_id.as_deref() == Some(parent_id)).collect())
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("DELETE FROM sessions WHERE id = ?")?;
            stmt.execute([id])?;
            Ok(())
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteTranscriptLog
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteTranscriptLog {
    db: SqliteDb,
}

impl SqliteTranscriptLog {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl TranscriptLog for SqliteTranscriptLog {
    fn append(&self, record: &TranscriptRecord) -> Result<()> {
        let json = serde_json::to_string(record)
            .map_err(|e| DomainError::adapter("serialize transcript record", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO transcripts (session_id, at, data) VALUES (?, ?, ?)"
            )?;
            stmt.execute(params![record.session_id, record.at.to_rfc3339(), json])?;
            Ok(())
        })
    }

    fn read(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<TranscriptRecord>> {
        self.db.with_conn(|conn| {
            let query = match limit {
                Some(n) => format!("SELECT data FROM transcripts WHERE session_id = ? ORDER BY id DESC LIMIT {}", n),
                None => "SELECT data FROM transcripts WHERE session_id = ? ORDER BY id ASC".to_string(),
            };
            let mut stmt = conn.prepare(&query)?;
            let mut records: Vec<TranscriptRecord> = stmt.query_map([session_id], |row| {
                let data: String = row.get(0)?;
                let rec: TranscriptRecord = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(rec)
            })?
            .filter_map(|r| r.ok())
            .collect();

            if limit.is_some() {
                records.reverse();
            }
            Ok(records)
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteHarnessStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteHarnessStore {
    db: SqliteDb,
}

impl SqliteHarnessStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }

    pub fn search_fts(&self, query: &str, session_id: Option<&str>) -> Result<Vec<HarnessEntry>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                r#"
                SELECT h.data
                FROM harness_fts f
                JOIN harness_entries h ON f.name = h.name AND (f.session_id = h.session_id OR (f.session_id IS NULL AND h.session_id IS NULL))
                WHERE harness_fts MATCH ?
                  AND (h.scope = 'global' OR h.session_id = ?)
                ORDER BY rank
                "#
            )?;
            let entries = stmt.query_map(params![query, session_id], |row| {
                let data: String = row.get(0)?;
                let entry: HarnessEntry = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(entry)
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(entries)
        })
    }
}

impl HarnessStore for SqliteHarnessStore {
    fn upsert(&self, session_id: Option<&str>, entry: &HarnessEntry) -> Result<()> {
        let json = serde_json::to_string(entry)
            .map_err(|e| DomainError::adapter("serialize harness entry", e))?;
        let scope_str = match entry.scope {
            HarnessScope::Global => "global",
            HarnessScope::Local => "local",
        };
        self.db.with_conn(|conn| {
            let tx = conn.transaction()?;
            {
                let mut stmt = tx.prepare_cached(
                    r#"
                    INSERT INTO harness_entries (session_id, scope, name, data)
                    VALUES (?, ?, ?, ?)
                    ON CONFLICT(scope, session_id, name) DO UPDATE SET data = excluded.data
                    "#
                )?;
                stmt.execute(params![session_id, scope_str, entry.name, json])?;
            }
            {
                let mut fts = tx.prepare_cached(
                    "INSERT INTO harness_fts (name, body, session_id, scope) VALUES (?, ?, ?, ?)"
                )?;
                fts.execute(params![entry.name, entry.body, session_id, scope_str])?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    fn get(&self, session_id: Option<&str>, scope: HarnessScope, name: &str) -> Result<HarnessEntry> {
        let scope_str = match scope {
            HarnessScope::Global => "global",
            HarnessScope::Local => "local",
        };
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM harness_entries WHERE scope = ? AND name = ? AND (session_id = ? OR (? IS NULL AND session_id IS NULL))"
            )?;
            let mut rows = stmt.query(params![scope_str, name, session_id, session_id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let entry: HarnessEntry = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(entry)
            } else {
                Err(rusqlite::Error::ToSqlConversionFailure(Box::new(DomainError::not_found("harness entry", name))))
            }
        })
    }

    fn remove(&self, session_id: Option<&str>, scope: HarnessScope, name: &str) -> Result<()> {
        let scope_str = match scope {
            HarnessScope::Global => "global",
            HarnessScope::Local => "local",
        };
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "DELETE FROM harness_entries WHERE scope = ? AND name = ? AND (session_id = ? OR (? IS NULL AND session_id IS NULL))"
            )?;
            stmt.execute(params![scope_str, name, session_id, session_id])?;
            Ok(())
        })
    }

    fn list(&self, session_id: Option<&str>) -> Result<Vec<HarnessEntry>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM harness_entries WHERE scope = 'global' OR (session_id = ? AND session_id IS NOT NULL) ORDER BY name ASC"
            )?;
            let entries = stmt.query_map([session_id], |row| {
                let data: String = row.get(0)?;
                let entry: HarnessEntry = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(entry)
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(entries)
        })
    }

    fn record_refinement(&self, session_id: Option<&str>, refinement: &Refinement) -> Result<()> {
        let json = serde_json::to_string(refinement)
            .map_err(|e| DomainError::adapter("serialize refinement", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO refinements (id, session_id, created_at, data) VALUES (?, ?, ?, ?)"
            )?;
            stmt.execute(params![refinement.id, session_id, refinement.created_at.to_rfc3339(), json])?;
            Ok(())
        })
    }

    fn update_refinement(&self, session_id: Option<&str>, refinement: &Refinement) -> Result<()> {
        let json = serde_json::to_string(refinement)
            .map_err(|e| DomainError::adapter("serialize refinement", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "UPDATE refinements SET data = ? WHERE id = ? AND (session_id = ? OR (? IS NULL AND session_id IS NULL))"
            )?;
            stmt.execute(params![json, refinement.id, session_id, session_id])?;
            Ok(())
        })
    }

    fn refinements(&self, session_id: Option<&str>, limit: Option<usize>) -> Result<Vec<Refinement>> {
        self.db.with_conn(|conn| {
            let query = match limit {
                Some(n) => format!(
                    "SELECT data FROM refinements WHERE (session_id = ? OR (? IS NULL AND session_id IS NULL)) ORDER BY created_at DESC LIMIT {}",
                    n
                ),
                None => "SELECT data FROM refinements WHERE (session_id = ? OR (? IS NULL AND session_id IS NULL)) ORDER BY created_at ASC".to_string(),
            };
            let mut stmt = conn.prepare(&query)?;
            let refs = stmt.query_map(params![session_id, session_id], |row| {
                let data: String = row.get(0)?;
                let r: Refinement = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(r)
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(refs)
        })
    }

    fn refinement(&self, session_id: Option<&str>, id: &str) -> Result<Refinement> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM refinements WHERE id = ? AND (session_id = ? OR (? IS NULL AND session_id IS NULL))"
            )?;
            let mut rows = stmt.query(params![id, session_id, session_id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let r: Refinement = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(r)
            } else {
                Err(rusqlite::Error::ToSqlConversionFailure(Box::new(DomainError::not_found("refinement", id))))
            }
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteGoalStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteGoalStore {
    db: SqliteDb,
}

impl SqliteGoalStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl GoalStore for SqliteGoalStore {
    fn put(&self, goal: &Goal) -> Result<()> {
        let json = serde_json::to_string(goal)
            .map_err(|e| DomainError::adapter("serialize goal", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                r#"
                INSERT INTO goals (session_id, status, data) VALUES (?, ?, ?)
                ON CONFLICT(session_id) DO UPDATE SET status = excluded.status, data = excluded.data
                "#
            )?;
            stmt.execute(params![goal.session_id, serde_json::to_string(&goal.status).unwrap_or_default(), json])?;
            Ok(())
        })
    }

    fn get(&self, session_id: &str) -> Result<Option<Goal>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM goals WHERE session_id = ?")?;
            let mut rows = stmt.query([session_id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let goal: Goal = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Some(goal))
            } else {
                Ok(None)
            }
        })
    }

    fn clear(&self, session_id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("DELETE FROM goals WHERE session_id = ?")?;
            stmt.execute([session_id])?;
            Ok(())
        })
    }

    fn active(&self) -> Result<Vec<Goal>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM goals")?;
            let goals = stmt.query_map([], |row| {
                let data: String = row.get(0)?;
                let g: Goal = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(g)
            })?
            .filter_map(|r| r.ok())
            .filter(|g| g.status == GoalStatus::Active)
            .collect();
            Ok(goals)
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteHeartbeatStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteHeartbeatStore {
    db: SqliteDb,
}

impl SqliteHeartbeatStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl HeartbeatStore for SqliteHeartbeatStore {
    fn put(&self, heartbeat: &Heartbeat) -> Result<()> {
        let json = serde_json::to_string(heartbeat)
            .map_err(|e| DomainError::adapter("serialize heartbeat", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                r#"
                INSERT INTO heartbeats (id, session_id, next_fire_at, data) VALUES (?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET next_fire_at = excluded.next_fire_at, data = excluded.data
                "#
            )?;
            stmt.execute(params![heartbeat.id, heartbeat.session_id, heartbeat.next_fire_at.to_rfc3339(), json])?;
            Ok(())
        })
    }

    fn get(&self, id: &str) -> Result<Heartbeat> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM heartbeats WHERE id = ?")?;
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let h: Heartbeat = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(h)
            } else {
                Err(rusqlite::Error::ToSqlConversionFailure(Box::new(DomainError::not_found("heartbeat", id))))
            }
        })
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("DELETE FROM heartbeats WHERE id = ?")?;
            stmt.execute([id])?;
            Ok(())
        })
    }

    fn list(&self, session_id: Option<&str>) -> Result<Vec<Heartbeat>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM heartbeats WHERE (? IS NULL OR session_id = ?) ORDER BY next_fire_at ASC"
            )?;
            let beats = stmt.query_map(params![session_id, session_id], |row| {
                let data: String = row.get(0)?;
                let h: Heartbeat = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(h)
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(beats)
        })
    }

    fn due(&self, now: DateTime<Utc>) -> Result<Vec<Heartbeat>> {
        let all = self.list(None)?;
        Ok(all.into_iter().filter(|h| h.is_due(now)).collect())
    }
}

// -----------------------------------------------------------------------------
// SqliteScheduleStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteScheduleStore {
    db: SqliteDb,
}

impl SqliteScheduleStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl ScheduleStore for SqliteScheduleStore {
    fn put(&self, job: &ScheduledJob) -> Result<()> {
        let json = serde_json::to_string(job)
            .map_err(|e| DomainError::adapter("serialize scheduled job", e))?;
        let next_fire = job.next_tick.map(|t| t.to_rfc3339()).unwrap_or_default();
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                r#"
                INSERT INTO schedules (id, session_id, next_fire_at, data) VALUES (?, ?, ?, ?)
                ON CONFLICT(id) DO UPDATE SET next_fire_at = excluded.next_fire_at, data = excluded.data
                "#
            )?;
            stmt.execute(params![job.id, job.target, next_fire, json])?;
            Ok(())
        })
    }

    fn get(&self, id: &str) -> Result<ScheduledJob> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM schedules WHERE id = ?")?;
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let j: ScheduledJob = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(j)
            } else {
                Err(rusqlite::Error::ToSqlConversionFailure(Box::new(DomainError::not_found("schedule", id))))
            }
        })
    }

    fn list(&self, target: Option<&str>, include_finished: bool) -> Result<Vec<ScheduledJob>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM schedules WHERE (? IS NULL OR session_id = ?) ORDER BY next_fire_at ASC"
            )?;
            let jobs = stmt.query_map(params![target, target], |row| {
                let data: String = row.get(0)?;
                let j: ScheduledJob = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(j)
            })?
            .filter_map(|r| r.ok())
            .filter(|j| include_finished || !(j.status == JobStatus::Completed || j.status == JobStatus::Cancelled || j.status == JobStatus::Failed))
            .collect();
            Ok(jobs)
        })
    }

    fn due(&self, now: DateTime<Utc>) -> Result<Vec<ScheduledJob>> {
        let all = self.list(None, false)?;
        Ok(all.into_iter().filter(|j| j.is_due(now)).collect())
    }

    fn remove(&self, id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("DELETE FROM schedules WHERE id = ?")?;
            stmt.execute([id])?;
            Ok(())
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteMessageStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteMessageStore {
    db: SqliteDb,
}

impl SqliteMessageStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl MessageStore for SqliteMessageStore {
    fn enqueue(&self, message: &AgentMessage) -> Result<()> {
        let json = serde_json::to_string(message)
            .map_err(|e| DomainError::adapter("serialize message", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO messages (id, sender_session_id, receiver_session_id, data) VALUES (?, ?, ?, ?)"
            )?;
            stmt.execute(params![message.id, message.sender_session_id, message.receiver_session_id, json])?;
            Ok(())
        })
    }

    fn update(&self, message: &AgentMessage) -> Result<()> {
        let json = serde_json::to_string(message)
            .map_err(|e| DomainError::adapter("serialize message", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("UPDATE messages SET data = ? WHERE id = ?")?;
            stmt.execute(params![json, message.id])?;
            Ok(())
        })
    }

    fn pending_for(&self, session_id: &str) -> Result<Vec<AgentMessage>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM messages WHERE receiver_session_id = ? ORDER BY id ASC"
            )?;
            let msgs = stmt.query_map([session_id], |row| {
                let data: String = row.get(0)?;
                let m: AgentMessage = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(m)
            })?
            .filter_map(|r| r.ok())
            .filter(|m| m.is_pending())
            .collect();
            Ok(msgs)
        })
    }

    fn outbox(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<AgentMessage>> {
        self.db.with_conn(|conn| {
            let query = match limit {
                Some(n) => format!(
                    "SELECT data FROM messages WHERE sender_session_id = ? ORDER BY id DESC LIMIT {}",
                    n
                ),
                None => "SELECT data FROM messages WHERE sender_session_id = ? ORDER BY id ASC".to_string(),
            };
            let mut stmt = conn.prepare(&query)?;
            let msgs = stmt.query_map([session_id], |row| {
                let data: String = row.get(0)?;
                let m: AgentMessage = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(m)
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(msgs)
        })
    }

    fn get(&self, id: &str) -> Result<AgentMessage> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM messages WHERE id = ?")?;
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let m: AgentMessage = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(m)
            } else {
                Err(rusqlite::Error::ToSqlConversionFailure(Box::new(DomainError::not_found("message", id))))
            }
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteSubagentRegistry
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteSubagentRegistry {
    db: SqliteDb,
}

impl SqliteSubagentRegistry {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl SubagentRegistry for SqliteSubagentRegistry {
    fn insert(&self, child: &Subagent) -> Result<()> {
        let json = serde_json::to_string(child)
            .map_err(|e| DomainError::adapter("serialize subagent", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO subagents (child_id, parent_session_id, name, data) VALUES (?, ?, ?, ?)"
            )?;
            stmt.execute(params![child.child_id, child.parent_session_id, child.name, json])?;
            Ok(())
        })
    }

    fn update(&self, child: &Subagent) -> Result<()> {
        let json = serde_json::to_string(child)
            .map_err(|e| DomainError::adapter("serialize subagent", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("UPDATE subagents SET data = ? WHERE child_id = ?")?;
            stmt.execute(params![json, child.child_id])?;
            Ok(())
        })
    }

    fn get(&self, parent_session_id: &str, selector: &str) -> Result<Subagent> {
        let children = self.list(parent_session_id, true)?;
        children
            .into_iter()
            .find(|c| c.matches_selector(selector))
            .ok_or_else(|| DomainError::not_found("subagent", selector))
    }

    fn list(&self, parent_session_id: &str, include_deleted: bool) -> Result<Vec<Subagent>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM subagents WHERE parent_session_id = ? ORDER BY id ASC"
            )?;
            let subs = stmt.query_map([parent_session_id], |row| {
                let data: String = row.get(0)?;
                let s: Subagent = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(s)
            })?
            .filter_map(|r| r.ok())
            .filter(|s| include_deleted || s.status != SubagentStatus::Deleted)
            .collect();
            Ok(subs)
        })
    }

    fn all(&self) -> Result<Vec<Subagent>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM subagents ORDER BY id ASC")?;
            let subs = stmt.query_map([], |row| {
                let data: String = row.get(0)?;
                let s: Subagent = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(s)
            })?
            .filter_map(|r| r.ok())
            .collect();
            Ok(subs)
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteAutonomousStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteAutonomousStore {
    db: SqliteDb,
}

impl SqliteAutonomousStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl AutonomousStore for SqliteAutonomousStore {
    fn put(&self, state: &AutonomousState) -> Result<()> {
        let json = serde_json::to_string(state)
            .map_err(|e| DomainError::adapter("serialize autonomous state", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                r#"
                INSERT INTO autonomous_states (session_id, data) VALUES (?, ?)
                ON CONFLICT(session_id) DO UPDATE SET data = excluded.data
                "#
            )?;
            stmt.execute(params![state.session_id, json])?;
            Ok(())
        })
    }

    fn get(&self, session_id: &str) -> Result<Option<AutonomousState>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM autonomous_states WHERE session_id = ?")?;
            let mut rows = stmt.query([session_id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let state: AutonomousState = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Some(state))
            } else {
                Ok(None)
            }
        })
    }

    fn clear(&self, session_id: &str) -> Result<()> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("DELETE FROM autonomous_states WHERE session_id = ?")?;
            stmt.execute([session_id])?;
            Ok(())
        })
    }
}

// -----------------------------------------------------------------------------
// SqliteCompactionStore
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteCompactionStore {
    db: SqliteDb,
}

impl SqliteCompactionStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl CompactionStore for SqliteCompactionStore {
    fn put(&self, state: &CompactionState) -> Result<()> {
        let json = serde_json::to_string(state)
            .map_err(|e| DomainError::adapter("serialize compaction state", e))?;
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                r#"
                INSERT INTO compaction_states (session_id, data) VALUES (?, ?)
                ON CONFLICT(session_id) DO UPDATE SET data = excluded.data
                "#
            )?;
            stmt.execute(params![state.session_id, json])?;
            Ok(())
        })
    }

    fn get(&self, session_id: &str) -> Result<Option<CompactionState>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM compaction_states WHERE session_id = ?")?;
            let mut rows = stmt.query([session_id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let state: CompactionState = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Some(state))
            } else {
                Ok(None)
            }
        })
    }
}

// -----------------------------------------------------------------------------
// Lineage Store (Autonomous Evolutionary Search)
// -----------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct SqliteLineageStore {
    db: SqliteDb,
}

impl SqliteLineageStore {
    pub fn new(db: SqliteDb) -> Self {
        Self { db }
    }
}

impl LineageStore for SqliteLineageStore {
    fn append(&self, entry: &umoja_domain::lineage::LineageEntry) -> Result<()> {
        let json = serde_json::to_string(entry).map_err(|e| DomainError::adapter("serialize lineage entry", e))?;
        let created = entry.created_at.to_rfc3339();
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "INSERT INTO lineage_entries (id, target, generation, commit_hash, parent_id, rationale, data, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)"
            )?;
            stmt.execute(params![
                entry.id,
                entry.target,
                entry.generation as i64,
                entry.commit_hash,
                entry.parent_id,
                entry.rationale,
                json,
                created,
            ])?;
            Ok(())
        })
    }

    fn list(&self, target: &str, limit: usize) -> Result<Vec<umoja_domain::lineage::LineageEntry>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached(
                "SELECT data FROM lineage_entries WHERE target = ? ORDER BY generation DESC LIMIT ?"
            )?;
            let mut rows = stmt.query(params![target, limit as i64])?;
            let mut entries = Vec::new();
            while let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                if let Ok(entry) = serde_json::from_str(&data) {
                    entries.push(entry);
                }
            }
            Ok(entries)
        })
    }

    fn pareto_frontier(&self, target: &str) -> Result<umoja_domain::lineage::ParetoFrontier> {
        let all_entries = self.list(target, 1000)?;
        let mut frontier = umoja_domain::lineage::ParetoFrontier::new();
        for entry in all_entries.into_iter().rev() {
            frontier.update(entry);
        }
        Ok(frontier)
    }

    fn get(&self, id: &str) -> Result<Option<umoja_domain::lineage::LineageEntry>> {
        self.db.with_conn(|conn| {
            let mut stmt = conn.prepare_cached("SELECT data FROM lineage_entries WHERE id = ?")?;
            let mut rows = stmt.query([id])?;
            if let Some(row) = rows.next()? {
                let data: String = row.get(0)?;
                let entry: umoja_domain::lineage::LineageEntry = serde_json::from_str(&data)
                    .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
                Ok(Some(entry))
            } else {
                Ok(None)
            }
        })
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use umoja_domain::harness::EntryKind;
    use umoja_domain::session::{SessionKind, SessionStatus};

    #[test]
    fn session_store_crud() {
        let db = SqliteDb::in_memory().unwrap();
        let store = SqliteSessionStore::new(db);

        let s1 = Session {
            id: "ses-1".into(),
            name: "ses1-name".into(),
            kind: SessionKind::Root,
            status: SessionStatus::Idle,
            workdir: "/tmp/ses1".into(),
            runner: "claude".into(),
            model: None,
            parent_id: None,
            depth: 0,
            pid: None,
            usage: Default::default(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        store.insert(&s1).unwrap();

        let loaded = store.get("ses-1").unwrap();
        assert_eq!(loaded.id, "ses-1");
        assert_eq!(loaded.workdir, "/tmp/ses1");

        let resolved = store.resolve("ses1-name").unwrap();
        assert_eq!(resolved.id, "ses-1");
    }

    #[test]
    fn harness_store_and_fts5_search() {
        let db = SqliteDb::in_memory().unwrap();
        let store = SqliteHarnessStore::new(db);

        let entry1 = HarnessEntry::new(
            "entry-1",
            EntryKind::Memory,
            HarnessScope::Global,
            "arch-guide",
            "Clean architecture using pure rust and sqlite",
            "User request",
            Utc::now(),
        )
        .unwrap();
        store.upsert(None, &entry1).unwrap();

        let list = store.list(None).unwrap();
        assert_eq!(list.len(), 1);

        let results = store.search_fts("architecture", None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "arch-guide");
    }
}
