//! Filing defects against the tooling, and recording why an agent acted.
//!
//! Two append-only journals, both reachable from inside a kernel script so
//! an agent can use them at the moment it learns something rather than
//! hoping to remember at the end of a turn:
//!
//! * **reports** — a defect, papercut or missing capability in umoja
//!   itself, addressed to whoever maintains it.  These outlive the session
//!   that found them, so they live beside the umoja home rather than in a
//!   session transcript.
//! * **actions** — what an agent did and *why*.  A diff records the first
//!   and destroys the second; a reviewer needs both.
//!
//! Neither journal ever sends anything anywhere.  Filing is local; opening
//! an issue against the repository is a separate, deliberate act by a
//! person who has read what accumulated.

use std::io::Write;
use std::path::PathBuf;

use chrono::Utc;
use rhai::{Array, Dynamic, Engine, Map};
use serde::{Deserialize, Serialize};

use umoja_domain::report::{Report, ReportKind};

/// A redirect for tests, so a test never writes into the developer's real
/// umoja home.  Mutating `$UMOJA_HOME` would be the obvious way and is not
/// available: this crate forbids unsafe, and `set_var` is unsafe because it
/// races every other thread reading the environment.
static JOURNAL_OVERRIDE: std::sync::RwLock<Option<PathBuf>> = std::sync::RwLock::new(None);

/// Where the journals live: a test override, else `$UMOJA_HOME`, else
/// `~/.umoja`.
///
/// Deliberately not the working directory — a report about a broken
/// builtin belongs to the tool, not to whichever project happened to trip
/// over it.
fn journal_dir() -> PathBuf {
    if let Ok(guard) = JOURNAL_OVERRIDE.read() {
        if let Some(dir) = guard.as_ref() {
            return dir.clone();
        }
    }
    if let Some(explicit) = std::env::var_os("UMOJA_HOME") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".umoja")
}

fn reports_path() -> PathBuf {
    journal_dir().join("reports.jsonl")
}

fn actions_path() -> PathBuf {
    journal_dir().join("actions.jsonl")
}

fn append_line(path: &PathBuf, line: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    writeln!(file, "{line}").map_err(|e| format!("write {}: {e}", path.display()))
}

/// The agent's own name, so a pattern across sessions is attributable.
fn agent_name() -> String {
    std::env::var("UMOJA_AGENT")
        .or_else(|_| std::env::var("CLAUDE_AGENT"))
        .unwrap_or_else(|_| "agent".to_string())
}

fn session_id() -> Option<String> {
    std::env::var("UMOJA_SESSION_ID").ok()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ActionRecord {
    at: String,
    agent: String,
    session_id: Option<String>,
    action: String,
    target: Option<String>,
    why: String,
}

fn err(msg: String) -> Dynamic {
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(false));
    map.insert("error".into(), Dynamic::from(msg));
    Dynamic::from(map)
}

fn file_report(kind: &str, component: Option<&str>, title: &str, body: &str) -> Dynamic {
    let kind = match ReportKind::parse(kind) {
        Ok(k) => k,
        Err(e) => return err(e.to_string()),
    };
    let now = Utc::now();
    let id = format!("rep-{}", now.format("%Y%m%d%H%M%S%3f"));

    let report = match Report::new(
        id.clone(),
        kind,
        title,
        body,
        component.map(|c| c.to_string()),
        agent_name(),
        session_id(),
        now,
    ) {
        Ok(r) => r,
        Err(e) => return err(e.to_string()),
    };

    let line = match serde_json::to_string(&report) {
        Ok(l) => l,
        Err(e) => return err(format!("serialise report: {e}")),
    };
    if let Err(e) = append_line(&reports_path(), &line) {
        return err(e);
    }

    crate::activity::record_report(&id, kind.label(), &report.title);

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("id".into(), Dynamic::from(id));
    map.insert("kind".into(), Dynamic::from(kind.label().to_string()));
    map.insert(
        "path".into(),
        Dynamic::from(reports_path().display().to_string()),
    );
    Dynamic::from(map)
}

fn read_reports() -> Vec<Report> {
    let Ok(text) = std::fs::read_to_string(reports_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Report>(l).ok())
        .collect()
}

pub fn register_reporting_builtins(engine: &mut Engine) {
    // -------------------------------------------------------------------------
    // Filing defects against umoja itself
    // -------------------------------------------------------------------------
    engine.register_fn("report_bug", |title: &str, body: &str| -> Dynamic {
        file_report("bug", None, title, body)
    });
    engine.register_fn(
        "report_bug",
        |component: &str, title: &str, body: &str| -> Dynamic {
            file_report("bug", Some(component), title, body)
        },
    );
    engine.register_fn("report_error", |title: &str, body: &str| -> Dynamic {
        file_report("error", None, title, body)
    });
    engine.register_fn(
        "report_error",
        |component: &str, title: &str, body: &str| -> Dynamic {
            file_report("error", Some(component), title, body)
        },
    );
    engine.register_fn(
        "report",
        |kind: &str, component: &str, title: &str, body: &str| -> Dynamic {
            file_report(kind, Some(component), title, body)
        },
    );

    engine.register_fn("reports", || -> Dynamic {
        let mut arr = Array::new();
        for r in read_reports() {
            let mut m = Map::new();
            m.insert("id".into(), Dynamic::from(r.id.clone()));
            m.insert("kind".into(), Dynamic::from(r.kind.label().to_string()));
            m.insert("title".into(), Dynamic::from(r.title.clone()));
            m.insert("status".into(), Dynamic::from(r.status.label().to_string()));
            m.insert(
                "component".into(),
                Dynamic::from(r.component.clone().unwrap_or_default()),
            );
            arr.push(Dynamic::from(m));
        }
        Dynamic::from(arr)
    });

    // A markdown digest of everything filed, for pasting into an issue.
    engine.register_fn("reports_markdown", || -> String {
        let reports = read_reports();
        if reports.is_empty() {
            return "No reports filed.\n".to_string();
        }
        let mut out = String::from("# umoja reports\n\n");
        for r in reports {
            out.push_str(&r.to_markdown());
            out.push('\n');
        }
        out
    });

    // -------------------------------------------------------------------------
    // Recording what was done, and why
    // -------------------------------------------------------------------------
    engine.register_fn("log_action", |action: &str, why: &str| -> Dynamic {
        write_action(action, None, why)
    });
    engine.register_fn(
        "log_action",
        |action: &str, target: &str, why: &str| -> Dynamic {
            write_action(action, Some(target), why)
        },
    );

    engine.register_fn("actions", || -> Dynamic {
        let mut arr = Array::new();
        if let Ok(text) = std::fs::read_to_string(actions_path()) {
            for line in text.lines().filter(|l| !l.trim().is_empty()) {
                if let Ok(rec) = serde_json::from_str::<ActionRecord>(line) {
                    let mut m = Map::new();
                    m.insert("at".into(), Dynamic::from(rec.at));
                    m.insert("agent".into(), Dynamic::from(rec.agent));
                    m.insert("action".into(), Dynamic::from(rec.action));
                    m.insert(
                        "target".into(),
                        Dynamic::from(rec.target.unwrap_or_default()),
                    );
                    m.insert("why".into(), Dynamic::from(rec.why));
                    arr.push(Dynamic::from(m));
                }
            }
        }
        Dynamic::from(arr)
    });
}

fn write_action(action: &str, target: Option<&str>, why: &str) -> Dynamic {
    // A log line with no reason is the thing this exists to prevent, so it
    // is refused rather than written empty.
    if why.trim().is_empty() {
        return err("log_action needs a reason: say why, not just what".to_string());
    }
    if action.trim().is_empty() {
        return err("log_action needs an action".to_string());
    }

    let rec = ActionRecord {
        at: Utc::now().to_rfc3339(),
        agent: agent_name(),
        session_id: session_id(),
        action: action.trim().to_string(),
        target: target
            .map(|t| t.to_string())
            .filter(|t| !t.trim().is_empty()),
        why: why.trim().to_string(),
    };
    let line = match serde_json::to_string(&rec) {
        Ok(l) => l,
        Err(e) => return err(format!("serialise action: {e}")),
    };
    if let Err(e) = append_line(&actions_path(), &line) {
        return err(e);
    }

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert(
        "path".into(),
        Dynamic::from(actions_path().display().to_string()),
    );
    Dynamic::from(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Point both journals at a scratch directory so a test never writes
    /// into the developer's real umoja home.
    fn with_temp_home<T>(name: &str, f: impl FnOnce(&mut Engine) -> T) -> T {
        // One lock for the whole suite: the override is process-wide, so
        // two of these running at once would read each other's journals.
        static SERIALISE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _held = SERIALISE.lock().unwrap_or_else(|e| e.into_inner());

        let dir = std::env::temp_dir().join(format!("umoja_reporting_{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        *JOURNAL_OVERRIDE.write().unwrap() = Some(dir);

        let mut engine = Engine::new();
        register_reporting_builtins(&mut engine);
        let out = f(&mut engine);
        *JOURNAL_OVERRIDE.write().unwrap() = None;
        out
    }

    /// The point of the journal is that a maintainer can act on it, so a
    /// report with nothing reproducible in it never reaches disk.
    #[test]
    fn a_report_without_a_body_is_refused_and_nothing_is_written() {
        with_temp_home("empty_body", |engine| {
            let ok = engine
                .eval::<bool>(r#"report_bug("grep is broken", "   ").ok"#)
                .unwrap();
            assert!(!ok);
            assert!(
                !reports_path().exists(),
                "a refused report must not create the journal"
            );
        });
    }

    #[test]
    fn a_filed_report_is_readable_back_and_renders_as_markdown() {
        with_temp_home("roundtrip", |engine| {
            let ok = engine
                .eval::<bool>(
                    r#"report_bug("ast_rewrite", "rewrite left a broken file",
                                  "expected rollback, got a truncated file").ok"#,
                )
                .unwrap();
            assert!(ok);

            let count = engine.eval::<i64>("reports().len()").unwrap();
            assert_eq!(count, 1);

            let kind = engine.eval::<String>("reports()[0].kind").unwrap();
            assert_eq!(kind, "bug");
            let component = engine.eval::<String>("reports()[0].component").unwrap();
            assert_eq!(component, "ast_rewrite");

            let md = engine.eval::<String>("reports_markdown()").unwrap();
            assert!(md.contains("rewrite left a broken file"));
            assert!(md.contains("[bug]"));
        });
    }

    /// "What" without "why" is the failure mode this replaces, so the
    /// reason is mandatory rather than optional.
    #[test]
    fn an_action_logged_without_a_reason_is_refused() {
        with_temp_home("no_reason", |engine| {
            assert!(!engine
                .eval::<bool>(r#"log_action("edited lsp.rs", "  ").ok"#)
                .unwrap());
            assert!(engine
                .eval::<bool>(
                    r#"log_action("edited lsp.rs", "crates/x.rs",
                                      "guard reported success without checking").ok"#
                )
                .unwrap());

            let logged = engine.eval::<i64>("actions().len()").unwrap();
            assert_eq!(logged, 1, "only the action carrying a reason is kept");
            let why = engine.eval::<String>("actions()[0].why").unwrap();
            assert_eq!(why, "guard reported success without checking");
        });
    }
}
