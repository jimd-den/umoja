//! Embedded SQLite persistence engine and domain store implementations.

pub mod db;
pub mod schema;
pub mod stores;

pub use db::SqliteDb;
pub use stores::{
    SqliteAutonomousStore, SqliteCompactionStore, SqliteGoalStore, SqliteHarnessStore,
    SqliteHeartbeatStore, SqliteMessageStore, SqliteScheduleStore, SqliteSessionStore,
    SqliteSubagentRegistry, SqliteTranscriptLog,
};
