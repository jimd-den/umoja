//! SQLite database schema and initialization logic.

use rusqlite::{Connection, Result};

pub const SCHEMA_VERSION: i32 = 1;

pub fn initialize_schema(conn: &Connection) -> Result<()> {
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
    let _ = conn.pragma_update(None, "synchronous", "NORMAL");
    let _ = conn.pragma_update(None, "foreign_keys", "ON");

    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            applied_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id TEXT PRIMARY KEY,
            name TEXT,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            workdir TEXT NOT NULL,
            runner TEXT NOT NULL,
            status TEXT NOT NULL,
            data JSON NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_sessions_name ON sessions(name);

        CREATE TABLE IF NOT EXISTS transcripts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            at TEXT NOT NULL,
            data JSON NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_transcripts_session_at ON transcripts(session_id, at);

        CREATE TABLE IF NOT EXISTS harness_entries (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT,
            scope TEXT NOT NULL,
            name TEXT NOT NULL,
            data JSON NOT NULL,
            UNIQUE(scope, session_id, name)
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS harness_fts USING fts5(
            name,
            body,
            session_id UNINDEXED,
            scope UNINDEXED,
            tokenize='porter unicode61'
        );

        CREATE TABLE IF NOT EXISTS refinements (
            id TEXT PRIMARY KEY,
            session_id TEXT,
            created_at TEXT NOT NULL,
            data JSON NOT NULL
        );

        CREATE TABLE IF NOT EXISTS goals (
            id TEXT,
            session_id TEXT PRIMARY KEY,
            status TEXT NOT NULL,
            data JSON NOT NULL
        );

        CREATE TABLE IF NOT EXISTS heartbeats (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            next_fire_at TEXT NOT NULL,
            data JSON NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_heartbeats_due ON heartbeats(next_fire_at);

        CREATE TABLE IF NOT EXISTS schedules (
            id TEXT PRIMARY KEY,
            session_id TEXT NOT NULL,
            next_fire_at TEXT NOT NULL,
            data JSON NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_schedules_due ON schedules(next_fire_at);

        CREATE TABLE IF NOT EXISTS messages (
            id TEXT PRIMARY KEY,
            sender_session_id TEXT NOT NULL,
            receiver_session_id TEXT NOT NULL,
            data JSON NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_messages_receiver ON messages(receiver_session_id);

        CREATE TABLE IF NOT EXISTS subagents (
            child_id TEXT PRIMARY KEY,
            parent_session_id TEXT NOT NULL,
            name TEXT NOT NULL,
            data JSON NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_subagents_parent ON subagents(parent_session_id);

        CREATE TABLE IF NOT EXISTS autonomous_states (
            session_id TEXT PRIMARY KEY,
            data JSON NOT NULL
        );

        CREATE TABLE IF NOT EXISTS compaction_states (
            session_id TEXT PRIMARY KEY,
            data JSON NOT NULL
        );
        "#,
    )?;

    let mut stmt = conn.prepare("SELECT COUNT(*) FROM schema_migrations WHERE version = ?")?;
    let count: i64 = stmt.query_row([SCHEMA_VERSION], |row| row.get(0))?;
    if count == 0 {
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?, ?)",
            (SCHEMA_VERSION, now),
        )?;
    }

    Ok(())
}
