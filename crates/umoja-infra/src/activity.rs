//! Every umoja run, every file mutation, recorded without being asked.
//!
//! `log_action` and `report_bug` depend on an agent choosing to call them,
//! and an agent that has not read the current rules will not. This module
//! is the part that does not depend on cooperation: the CLI records each
//! invocation, and each editing builtin records what it touched, whether
//! anyone asked for it or not.
//!
//! Two consequences follow from that:
//!
//! * There is always a record of what an agent did, even a badly behaved
//!   or out-of-date one.
//! * The number of changes made since the last report is knowable, so the
//!   tool can *prompt* for a report rather than hoping for one.
//!
//! Nothing here may ever fail a command. Every function swallows its
//! errors: a broken journal is a lost record, never a broken build.

use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// How many unreported changes before the tool asks for a report.
pub const NUDGE_EVERY: i64 = 5;

/// A redirect for tests, so a test never writes into the real umoja home.
static DB_OVERRIDE: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

pub fn set_db_path_for_test(path: Option<PathBuf>) {
    if let Ok(mut guard) = DB_OVERRIDE.write() {
        *guard = path;
    }
}

/// The project this work belongs to: the nearest ancestor holding a
/// `.git`, else the working directory.
///
/// Activity is a fact about a project, not about the machine.  A single
/// global journal mixes every repo together, so `umoja activity` answers
/// the wrong question and the report nudge counts changes made somewhere
/// else entirely.
pub fn project_root() -> PathBuf {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut here = cwd.as_path();
    loop {
        if here.join(".git").exists() {
            return here.to_path_buf();
        }
        match here.parent() {
            Some(parent) => here = parent,
            None => break,
        }
    }
    cwd
}

fn db_path() -> PathBuf {
    if let Ok(guard) = DB_OVERRIDE.read() {
        if let Some(p) = guard.as_ref() {
            return p.clone();
        }
    }
    project_root().join(".umoja").join("activity.db")
}

/// Keep the journal out of the repository that hosts it.
///
/// The alternative is asking every project to add a line to its own
/// `.gitignore`, which will not happen reliably and turns a tool detail
/// into someone else's chore.  A directory that ignores itself needs no
/// cooperation from the repo it sits in.
fn ensure_self_ignored(dir: &Path) {
    let marker = dir.join(".gitignore");
    if !marker.exists() {
        let _ = std::fs::write(&marker, "# Created by umoja: this journal is local state.\n*\n");
    }
}

fn agent_name() -> String {
    std::env::var("UMOJA_AGENT")
        .or_else(|_| std::env::var("CLAUDE_AGENT"))
        .unwrap_or_else(|_| "agent".to_string())
}

fn session_id() -> Option<String> {
    std::env::var("UMOJA_SESSION_ID").ok()
}

/// Open the journal, creating it and its tables if needed.
///
/// Separate from the main schema on purpose: that one is not wired into
/// the live path, and this must work regardless.
fn open() -> Option<Connection> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok()?;
        ensure_self_ignored(parent);
    }
    let conn = Connection::open(&path).ok()?;
    let _ = conn.busy_timeout(std::time::Duration::from_millis(5000));
    let _ = conn.pragma_update(None, "journal_mode", "WAL");
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS runs (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            at TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_id TEXT,
            command TEXT NOT NULL,
            args TEXT NOT NULL,
            cwd TEXT NOT NULL,
            ok INTEGER NOT NULL,
            exit_code INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_runs_at ON runs(at);

        CREATE TABLE IF NOT EXISTS mutations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            at TEXT NOT NULL,
            agent TEXT NOT NULL,
            session_id TEXT,
            op TEXT NOT NULL,
            path TEXT NOT NULL,
            guarded INTEGER NOT NULL,
            checker TEXT NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_mutations_at ON mutations(at);

        CREATE TABLE IF NOT EXISTS nudge_state (
            key TEXT PRIMARY KEY,
            value INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS filed_reports (
            id TEXT PRIMARY KEY,
            at TEXT NOT NULL,
            kind TEXT NOT NULL,
            title TEXT NOT NULL
        );
        "#,
    )
    .ok()?;
    Some(conn)
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Record one invocation of the `umoja` binary.
pub fn record_run(command: &str, args: &str, ok: bool, exit_code: i32, duration_ms: u64) {
    let Some(conn) = open() else { return };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let _ = conn.execute(
        "INSERT INTO runs (at, agent, session_id, command, args, cwd, ok, exit_code, duration_ms)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            now(),
            agent_name(),
            session_id(),
            command,
            args,
            cwd,
            ok as i64,
            exit_code as i64,
            duration_ms as i64,
        ],
    );
}

/// Record one file mutation, guarded or not.
pub fn record_mutation(op: &str, path: &str, guarded: bool, checker: &str) {
    let Some(conn) = open() else { return };
    let _ = conn.execute(
        "INSERT INTO mutations (at, agent, session_id, op, path, guarded, checker)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        rusqlite::params![
            now(),
            agent_name(),
            session_id(),
            op,
            path,
            guarded as i64,
            checker,
        ],
    );
}

/// Record that a report was filed, which is what resets the nudge.
pub fn record_report(id: &str, kind: &str, title: &str) {
    let Some(conn) = open() else { return };
    let _ = conn.execute(
        "INSERT OR REPLACE INTO filed_reports (id, at, kind, title) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, now(), kind, title],
    );
    let _ = conn.execute(
        "INSERT OR REPLACE INTO nudge_state (key, value) VALUES ('last_nudged_at', 0)",
        [],
    );
}

/// How many files have been changed since the last report was filed.
pub fn unreported_changes() -> i64 {
    let Some(conn) = open() else { return 0 };
    conn.query_row(
        "SELECT COUNT(*) FROM mutations
         WHERE at > COALESCE((SELECT MAX(at) FROM filed_reports), '')",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// The warning to show, when enough has changed to be worth reporting on.
///
/// Deliberately phrased as a question rather than an order: most runs of
/// changes are fine, and a nag that cries wolf gets tuned out. It fires
/// once every [`NUDGE_EVERY`] changes, not on every one.
pub fn nudge() -> Option<String> {
    let n = unreported_changes();
    // Not `n % NUDGE_EVERY == 0`: a script that makes six changes between
    // two commands would step straight over the multiple and never be
    // asked at all.  What matters is how many have gone unmentioned since
    // the last time we asked.
    let since = n - last_nudged_at();
    if n == 0 || since < NUDGE_EVERY {
        return None;
    }
    remember_nudge(n);
    Some(format!(
        "umoja: {n} files changed since your last report.\n\
         \x20      If anything in umoja misbehaved — a wrong result, a guard that did not \
         guard, a step that took far more effort than it should have — file it now:\n\
         \x20        umoja kernel exec 'report_bug(\"component\", \"title\", \"Expected / Observed / Repro\")'\n\
         \x20      Nothing to report is a fine answer; this asks once every {NUDGE_EVERY} changes."
    ))
}

/// The change count when the tool last asked for a report.
fn last_nudged_at() -> i64 {
    let Some(conn) = open() else { return 0 };
    conn.query_row(
        "SELECT value FROM nudge_state WHERE key = 'last_nudged_at'",
        [],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

fn remember_nudge(count: i64) {
    let Some(conn) = open() else { return };
    let _ = conn.execute(
        "INSERT OR REPLACE INTO nudge_state (key, value) VALUES ('last_nudged_at', ?1)",
        [count],
    );
}

/// The most recent runs, for `umoja activity`.
pub fn recent_runs(limit: i64) -> Vec<(String, String, String, bool, i64)> {
    let Some(conn) = open() else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT at, command, args, ok, duration_ms FROM runs ORDER BY id DESC LIMIT ?1",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)? != 0,
            row.get::<_, i64>(4)?,
        ))
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// The most recent mutations, for `umoja activity --changes`.
pub fn recent_mutations(limit: i64) -> Vec<(String, String, String, bool, String)> {
    let Some(conn) = open() else { return Vec::new() };
    let Ok(mut stmt) = conn.prepare(
        "SELECT at, op, path, guarded, checker FROM mutations ORDER BY id DESC LIMIT ?1",
    ) else {
        return Vec::new();
    };
    let rows = stmt.query_map([limit], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)? != 0,
            row.get::<_, String>(4)?,
        ))
    });
    match rows {
        Ok(iter) => iter.filter_map(|r| r.ok()).collect(),
        Err(_) => Vec::new(),
    }
}

/// How many mutations went in unverified — the number worth worrying about.
pub fn unguarded_count() -> i64 {
    let Some(conn) = open() else { return 0 };
    conn.query_row("SELECT COUNT(*) FROM mutations WHERE guarded = 0", [], |r| {
        r.get(0)
    })
    .unwrap_or(0)
}

/// Convenience for the editing builtins, which know a `Path`.
pub fn record_path_mutation(op: &str, path: &Path, guarded: bool, checker: &str) {
    record_mutation(op, &path.display().to_string(), guarded, checker);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The db-path override is process-wide, so every test that touches it
    /// must hold this lock or it will read another test's journal.
    static SERIALISE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn serialised<T>(f: impl FnOnce() -> T) -> T {
        let _held = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());
        f()
    }

    fn with_temp_db<T>(name: &str, f: impl FnOnce() -> T) -> T {
        serialised(|| {
            let dir = std::env::temp_dir().join(format!("umoja_activity_{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            set_db_path_for_test(Some(dir.join("activity.db")));
            let out = f();
            set_db_path_for_test(None);
            out
        })
    }

    /// Two projects must not share a journal, or `umoja activity` answers
    /// about the wrong repo and the nudge counts changes made elsewhere.
    #[test]
    fn each_project_gets_its_own_journal() {
        serialised(|| {
            let base = std::env::temp_dir().join("umoja_project_scope");
            let _ = std::fs::remove_dir_all(&base);
            let alpha = base.join("alpha");
            let beta = base.join("beta");
            for p in [&alpha, &beta] {
                std::fs::create_dir_all(p.join(".git")).unwrap();
            }

            let a_db = alpha.join(".umoja").join("activity.db");
            let b_db = beta.join(".umoja").join("activity.db");
            assert_ne!(a_db, b_db);

            set_db_path_for_test(Some(a_db));
            record_mutation("try_edit", "alpha/src/main.rs", true, "cargo");
            assert_eq!(unreported_changes(), 1);

            set_db_path_for_test(Some(b_db));
            assert_eq!(
                unreported_changes(),
                0,
                "a change in one project must not show up in another"
            );
            set_db_path_for_test(None);
        })
    }

    /// The journal must not become the host repository's problem.
    #[test]
    fn the_journal_directory_ignores_itself() {
        serialised(|| {
            let dir = std::env::temp_dir().join("umoja_self_ignore");
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            set_db_path_for_test(Some(dir.join("activity.db")));
            record_run("kernel exec", "print(1)", true, 0, 1);
            set_db_path_for_test(None);

            let ignore = dir.join(".gitignore");
            assert!(ignore.exists(), "the journal directory must ignore itself");
            let body = std::fs::read_to_string(&ignore).unwrap();
            assert!(body.contains('*'), "it must ignore everything under it");
        })
    }

    /// A directory inside a git repository belongs to that repository, not
    /// to wherever the agent happened to be standing.
    #[test]
    fn the_project_root_is_the_git_root() {
        let base = std::env::temp_dir().join("umoja_git_root");
        let _ = std::fs::remove_dir_all(&base);
        let nested = base.join("crates").join("deep");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(base.join(".git")).unwrap();

        // The rule `project_root` applies, asserted without changing the
        // process directory out from under every other test.
        let mut here = nested.as_path();
        let found = loop {
            if here.join(".git").exists() {
                break here.to_path_buf();
            }
            match here.parent() {
                Some(p) => here = p,
                None => break nested.clone(),
            }
        };
        assert_eq!(found, base);
    }

    /// The point of this module: a run is recorded whether or not the
    /// agent cooperated.
    #[test]
    fn runs_are_recorded_without_being_asked() {
        with_temp_db("runs", || {
            record_run("kernel exec", "print(1)", true, 0, 12);
            record_run("goal status", "", true, 0, 3);
            let runs = recent_runs(10);
            assert_eq!(runs.len(), 2);
            // Newest first.
            assert_eq!(runs[0].1, "goal status");
            assert_eq!(runs[1].1, "kernel exec");
            assert!(runs[1].3, "a successful run is marked ok");
        });
    }

    #[test]
    fn an_unverified_mutation_is_recorded_as_such() {
        with_temp_db("mutations", || {
            record_mutation("try_edit", "src/a.rs", true, "cargo");
            record_mutation("write", "src/app.ts", false, "none");
            assert_eq!(unguarded_count(), 1);
            let muts = recent_mutations(10);
            assert_eq!(muts.len(), 2);
            assert_eq!(muts[0].2, "src/app.ts");
            assert!(!muts[0].3);
        });
    }

    /// The nudge fires once a batch has gone unmentioned, then goes quiet
    /// again, so it does not become noise that gets tuned out.
    #[test]
    fn the_nudge_fires_every_nth_unreported_change() {
        with_temp_db("nudge", || {
            assert!(nudge().is_none(), "nothing changed yet, nothing to ask");
            for i in 1..NUDGE_EVERY {
                record_mutation("try_edit", &format!("src/{i}.rs"), true, "cargo");
                assert!(nudge().is_none(), "quiet below the threshold");
            }
            record_mutation("try_edit", "src/last.rs", true, "cargo");
            let warning = nudge().expect("the threshold was reached");
            assert!(warning.contains("report_bug"));
            assert!(warning.contains(&format!("{NUDGE_EVERY} files changed")));

            assert!(nudge().is_none(), "it asks once, not continuously");
        });
    }

    /// A script that makes several changes between two commands steps over
    /// the exact multiple.  Asking must depend on how many went unmentioned,
    /// not on the count landing on a round number.
    #[test]
    fn the_nudge_still_fires_when_the_threshold_is_overshot() {
        with_temp_db("overshoot", || {
            for i in 0..(NUDGE_EVERY + 1) {
                record_mutation("write", &format!("src/{i}.rs"), false, "none");
            }
            assert_eq!(unreported_changes(), NUDGE_EVERY + 1);
            assert!(
                nudge().is_some(),
                "six changes with a threshold of five must still be asked about"
            );
        });
    }

    /// Filing a report is what clears the backlog; otherwise the tool
    /// would keep asking for something already delivered.
    #[test]
    fn a_filed_report_resets_the_count() {
        with_temp_db("reset", || {
            for i in 0..NUDGE_EVERY {
                record_mutation("try_edit", &format!("src/{i}.rs"), true, "cargo");
            }
            assert_eq!(unreported_changes(), NUDGE_EVERY);

            record_report("rep-1", "bug", "something misbehaved");
            assert_eq!(unreported_changes(), 0);
            assert!(nudge().is_none());
        });
    }
}

