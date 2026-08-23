//! Structural, syntax-aware search and rewrite backed by `ast-grep`.
//!
//! Line- and regex-based edits know nothing about syntax, so they match
//! inside strings and comments, miss a call that wrapped onto a second
//! line, and corrupt a file whenever the anchor was not where it was
//! assumed to be.  `ast-grep` matches the *parse tree* instead: a pattern
//! is code with metavariables, and it matches only where the grammar says
//! that shape occurs.
//!
//! Every rewrite here is guarded the way the `try_*` family is — written,
//! checked, and rolled back if the checker rejects it — so a structural
//! edit can never leave a broken file on disk.

use std::path::Path;
use std::process::Command;

use rhai::{Array, Dynamic, Engine, Map};
use serde_json::Value;

use super::lsp::run_diagnostics;

/// The binary that answers for structural queries.
const AST_GREP: &str = "ast-grep";

pub fn register_astgrep_builtins(engine: &mut Engine) {
    engine.register_fn("ast_grep_available", || -> Dynamic { availability() });

    engine.register_fn("ast_grep", |pattern: &str, path: &str| -> Dynamic {
        search(pattern, path, None)
    });
    engine.register_fn(
        "ast_grep",
        |pattern: &str, path: &str, lang: &str| -> Dynamic { search(pattern, path, Some(lang)) },
    );

    engine.register_fn(
        "ast_rewrite",
        |pattern: &str, rewrite: &str, path: &str| -> Dynamic {
            guarded_rewrite(pattern, rewrite, path, None)
        },
    );
    engine.register_fn(
        "ast_rewrite",
        |pattern: &str, rewrite: &str, path: &str, lang: &str| -> Dynamic {
            guarded_rewrite(pattern, rewrite, path, Some(lang))
        },
    );
}

/// Whether `ast-grep` is on the PATH, and how to get it when it is not.
///
/// An agent has to be able to *ask*: a missing binary and a pattern that
/// matched nothing otherwise produce the same empty result.
fn availability() -> Dynamic {
    let mut map = Map::new();
    let version = Command::new(AST_GREP)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());

    match version {
        Some(v) => {
            map.insert("installed".into(), Dynamic::from(true));
            map.insert("version".into(), Dynamic::from(v));
            map.insert("install_hint".into(), Dynamic::from(String::new()));
        }
        None => {
            map.insert("installed".into(), Dynamic::from(false));
            map.insert("version".into(), Dynamic::from(String::new()));
            map.insert(
                "install_hint".into(),
                Dynamic::from(
                    "ast-grep is not installed.  Ask the user before installing, then run one of: \
                     `cargo install ast-grep --locked`, `npm i -g @ast-grep/cli`, \
                     or `brew install ast-grep`."
                        .to_string(),
                ),
            );
        }
    }
    Dynamic::from(map)
}

fn missing_binary(extra: &[(&str, Dynamic)]) -> Dynamic {
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(false));
    map.insert("available".into(), Dynamic::from(false));
    map.insert(
        "error".into(),
        Dynamic::from(
            "ast-grep is not installed; call ast_grep_available() and ask the user to install it."
                .to_string(),
        ),
    );
    for (k, v) in extra {
        map.insert((*k).into(), v.clone());
    }
    Dynamic::from(map)
}

fn installed() -> bool {
    Command::new(AST_GREP)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn with_lang(cmd: &mut Command, lang: Option<&str>) {
    if let Some(l) = lang {
        cmd.arg("--lang").arg(l);
    }
}

fn error_map(msg: &str) -> Dynamic {
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(false));
    map.insert("error".into(), Dynamic::from(msg.to_string()));
    Dynamic::from(map)
}

/// Matches for `pattern` under `path`, as a list of located hits.
fn search(pattern: &str, path: &str, lang: Option<&str>) -> Dynamic {
    if !installed() {
        return missing_binary(&[("matches", Dynamic::from(Array::new()))]);
    }

    let mut cmd = Command::new(AST_GREP);
    cmd.arg("run")
        .arg("--pattern")
        .arg(pattern)
        .arg("--json=compact");
    with_lang(&mut cmd, lang);
    cmd.arg(path);

    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => return error_map(&format!("could not run ast-grep: {e}")),
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let parsed: Vec<Value> = serde_json::from_str(stdout.trim()).unwrap_or_default();

    let mut matches = Array::new();
    for hit in parsed {
        let mut m = Map::new();
        m.insert(
            "file".into(),
            Dynamic::from(
                hit.get("file")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            ),
        );
        let range = hit.get("range");
        let start = range.and_then(|r| r.get("start"));
        let end = range.and_then(|r| r.get("end"));
        // ast-grep counts lines from zero; every other umoja file call is
        // 1-indexed, so the boundary is normalised here rather than in
        // every script that uses it.
        m.insert(
            "line".into(),
            Dynamic::from(
                start
                    .and_then(|s| s.get("line"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    + 1,
            ),
        );
        m.insert(
            "column".into(),
            Dynamic::from(
                start
                    .and_then(|s| s.get("column"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    + 1,
            ),
        );
        m.insert(
            "end_line".into(),
            Dynamic::from(
                end.and_then(|s| s.get("line"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    + 1,
            ),
        );
        m.insert(
            "text".into(),
            Dynamic::from(
                hit.get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            ),
        );
        matches.push(Dynamic::from(m));
    }

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("available".into(), Dynamic::from(true));
    map.insert("count".into(), Dynamic::from(matches.len() as i64));
    map.insert("matches".into(), Dynamic::from(matches));
    Dynamic::from(map)
}

/// Apply `rewrite` to every match of `pattern`, then check the result and
/// roll the file back if the checker rejects it.
fn guarded_rewrite(pattern: &str, rewrite: &str, path: &str, lang: Option<&str>) -> Dynamic {
    if !installed() {
        return missing_binary(&[]);
    }

    let original = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return error_map(&format!("could not read {path}: {e}")),
    };

    let mut cmd = Command::new(AST_GREP);
    cmd.arg("run")
        .arg("--pattern")
        .arg(pattern)
        .arg("--rewrite")
        .arg(rewrite)
        .arg("--update-all");
    with_lang(&mut cmd, lang);
    cmd.arg(path);

    let out = match cmd.output() {
        Ok(o) => o,
        Err(e) => return error_map(&format!("could not run ast-grep: {e}")),
    };
    if !out.status.success() {
        return error_map(String::from_utf8_lossy(&out.stderr).trim());
    }

    let updated = std::fs::read_to_string(path).unwrap_or_else(|_| original.clone());
    if updated == original {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(true));
        map.insert("available".into(), Dynamic::from(true));
        map.insert("changed".into(), Dynamic::from(false));
        map.insert("errors".into(), Dynamic::from(Array::new()));
        return Dynamic::from(map);
    }

    // The same contract as `try_replace_lines`: a rewrite that does not
    // compile never survives on disk.
    let report = run_diagnostics(Path::new(path));
    if !report.ok {
        let _ = std::fs::write(path, &original);
        let mut errs = Array::new();
        for e in report.errors {
            let mut em = Map::new();
            em.insert("line".into(), Dynamic::from(e.line as i64));
            em.insert("message".into(), Dynamic::from(e.message));
            errs.push(Dynamic::from(em));
        }
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("available".into(), Dynamic::from(true));
        map.insert("changed".into(), Dynamic::from(false));
        map.insert("rolled_back".into(), Dynamic::from(true));
        map.insert("errors".into(), Dynamic::from(errs));
        return Dynamic::from(map);
    }

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("available".into(), Dynamic::from(true));
    map.insert("changed".into(), Dynamic::from(true));
    map.insert("errors".into(), Dynamic::from(Array::new()));
    Dynamic::from(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A missing binary and a pattern that matched nothing must not look
    /// alike; `ast_grep_available` is how an agent tells them apart.
    #[test]
    fn availability_reports_whether_ast_grep_is_installed() {
        let mut engine = Engine::new();
        register_astgrep_builtins(&mut engine);

        let installed_here = installed();
        assert_eq!(
            engine
                .eval::<bool>("ast_grep_available().installed")
                .unwrap(),
            installed_here
        );

        if !installed_here {
            let hint = engine
                .eval::<String>("ast_grep_available().install_hint")
                .unwrap();
            assert!(hint.contains("ast-grep"));
        }
    }

    /// A structural search finds a function by its shape, and reports it
    /// with 1-indexed coordinates like every other umoja file call.
    #[test]
    fn search_finds_a_function_by_shape() {
        if !installed() {
            return;
        }
        let dir = std::env::temp_dir().join("umoja_astgrep_search");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("sample.rs");
        std::fs::write(
            &file,
            "fn a() {}\n\nfn wanted(x: i32) -> i32 {\n    x + 1\n}\n",
        )
        .unwrap();

        let mut engine = Engine::new();
        register_astgrep_builtins(&mut engine);
        let target = file.display().to_string();
        engine.register_fn("target", move || target.clone());

        let found = engine
            .eval::<i64>("ast_grep(\"fn wanted($P) -> $R { $$$B }\", target()).count")
            .unwrap();
        assert_eq!(found, 1);

        let line = engine
            .eval::<i64>("ast_grep(\"fn wanted($P) -> $R { $$$B }\", target()).matches[0].line")
            .unwrap();
        assert_eq!(line, 3, "lines are reported 1-indexed");
    }

    /// The guard is the whole point: a rewrite that does not compile
    /// leaves the file exactly as it was.
    #[test]
    fn a_rewrite_that_breaks_the_file_is_rolled_back() {
        if !installed() {
            return;
        }
        let dir = std::env::temp_dir().join("umoja_astgrep_rollback");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("broken.rs");
        let original = "pub fn keep(x: i32) -> i32 {\n    x + 1\n}\n";
        std::fs::write(&file, original).unwrap();

        let mut engine = Engine::new();
        register_astgrep_builtins(&mut engine);
        let target = file.display().to_string();
        engine.register_fn("target", move || target.clone());

        let ok = engine
            .eval::<bool>("ast_rewrite(\"x + 1\", \"x +\", target(), \"rust\").ok")
            .unwrap();
        assert!(!ok, "a rewrite producing invalid Rust must be refused");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            original,
            "the file on disk must be untouched after a refused rewrite"
        );
    }
}
