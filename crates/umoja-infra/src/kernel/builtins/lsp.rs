//! LSP & Compiler Diagnostic Verification for Speculative Code Edits and Module Creation.

use std::path::{Path, PathBuf};
use std::process::Command;

use rhai::{Array, Dynamic, Engine, Map};
use serde_json::Value;

pub fn register_lsp_builtins(engine: &mut Engine) {
    // -------------------------------------------------------------------------
    // Diagnostic inspection
    // -------------------------------------------------------------------------
    engine.register_fn("lsp_check", |path: &str| -> Dynamic {
        let res = run_diagnostics(Path::new(path));
        to_dynamic_result(res)
    });

    engine.register_fn("lsp_diagnostics", || -> Dynamic {
        let res = run_diagnostics(Path::new("."));
        to_dynamic_result(res)
    });

    // -------------------------------------------------------------------------
    // Capability reporting — what can actually be checked here
    // -------------------------------------------------------------------------
    engine.register_fn("lsp_available", |path: &str| -> Dynamic {
        availability_for(Path::new(path))
    });

    engine.register_fn("capabilities", || -> Dynamic { toolchain_capabilities() });

    // -------------------------------------------------------------------------
    // Speculative, Guarded File Edits (Rejects on LSP/Compiler Error)
    // -------------------------------------------------------------------------
    engine.register_fn(
        "try_replace_lines",
        |path: &str, start: i64, end: i64, new_text: &str| -> Dynamic {
            try_speculative_replace_lines(
                path,
                start.max(1) as usize,
                end.max(1) as usize,
                new_text,
            )
        },
    );

    engine.register_fn(
        "try_edit",
        |path: &str, old_text: &str, new_text: &str| -> Dynamic {
            try_speculative_edit(path, old_text, new_text)
        },
    );

    engine.register_fn(
        "try_replace_fn",
        |path: &str, fn_name: &str, new_fn_body: &str| -> Dynamic {
            try_speculative_replace_fn(path, fn_name, new_fn_body)
        },
    );

    // -------------------------------------------------------------------------
    // Smart Module & New File Creation (Links parent mod & validates)
    // -------------------------------------------------------------------------
    engine.register_fn("create_module", |path: &str, content: &str| -> Dynamic {
        create_new_module(path, content, None)
    });

    engine.register_fn(
        "create_module",
        |path: &str, content: &str, parent_mod: &str| -> Dynamic {
            create_new_module(path, content, Some(parent_mod))
        },
    );
}

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub file: String,
    pub line: u32,
    pub column: u32,
    pub severity: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct DiagnosticReport {
    pub ok: bool,
    pub errors: Vec<Diagnostic>,
    pub warnings: Vec<Diagnostic>,
}

fn to_dynamic_result(report: DiagnosticReport) -> Dynamic {
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(report.ok));

    let mut err_arr = Array::new();
    for err in report.errors {
        let mut em = Map::new();
        em.insert("file".into(), Dynamic::from(err.file));
        em.insert("line".into(), Dynamic::from(err.line as i64));
        em.insert("column".into(), Dynamic::from(err.column as i64));
        em.insert("severity".into(), Dynamic::from(err.severity));
        em.insert("message".into(), Dynamic::from(err.message));
        err_arr.push(Dynamic::from(em));
    }
    map.insert("errors".into(), Dynamic::from(err_arr));

    let mut warn_arr = Array::new();
    for w in report.warnings {
        let mut wm = Map::new();
        wm.insert("file".into(), Dynamic::from(w.file));
        wm.insert("line".into(), Dynamic::from(w.line as i64));
        wm.insert("column".into(), Dynamic::from(w.column as i64));
        wm.insert("severity".into(), Dynamic::from(w.severity));
        wm.insert("message".into(), Dynamic::from(w.message));
        warn_arr.push(Dynamic::from(wm));
    }
    map.insert("warnings".into(), Dynamic::from(warn_arr));

    Dynamic::from(map)
}

/// The checker that will actually run for `target`, or `"none"`.
///
/// `run_diagnostics` answers `ok` for a file it does not know how to
/// check, which is indistinguishable from a file it checked and found
/// clean.  A guarded edit to a `.ts` or `.go` file would then report
/// success having verified nothing.  This says which tool will speak, so
/// a caller can tell silence from assent.
pub(crate) fn checker_for(target: &Path) -> &'static str {
    let ext = target.extension().and_then(|s| s.to_str()).unwrap_or("");
    if ext == "rs" || target.join("Cargo.toml").exists() || target == Path::new(".") {
        if is_inside_cargo_workspace(target) {
            "cargo"
        } else if ext == "rs" {
            "rustc"
        } else {
            "none"
        }
    } else if ext == "py" {
        "python3"
    } else if ext == "json" {
        "json"
    } else {
        "none"
    }
}

/// Stamp a result map with the checker that verified it.
///
/// A caller cannot otherwise distinguish "checked and clean" from "no
/// checker exists for this file type", and the second is not a guarantee.
pub(crate) fn note_guard(map: &mut Map, target: &Path) {
    note_guard_as("try_edit", map, target)
}

/// As [`note_guard`], naming the operation for the activity journal.
pub(crate) fn note_guard_as(op: &str, map: &mut Map, target: &Path) {
    note_written(target);
    let checker = checker_for(target);
    map.insert("checker".into(), Dynamic::from(checker.to_string()));
    map.insert("guarded".into(), Dynamic::from(checker != "none"));
    // Recorded here rather than at each call site: this runs on every kept
    // edit, and an agent that never calls `log_action` still leaves a trail.
    crate::activity::record_path_mutation(op, target, checker != "none", checker);
    if checker == "none" {
        map.insert(
            "warning".into(),
            Dynamic::from(
                "written WITHOUT verification: no checker is configured for this file type"
                    .to_string(),
            ),
        );
    }
}

/// Whether an edit to `target` will be verified, and by what.
fn availability_for(target: &Path) -> Dynamic {
    let checker = checker_for(target);
    let guarded = checker != "none";
    let mut map = Map::new();
    map.insert("checker".into(), Dynamic::from(checker.to_string()));
    map.insert("guarded".into(), Dynamic::from(guarded));
    map.insert(
        "note".into(),
        Dynamic::from(if guarded {
            format!("edits to this path are verified by `{checker}` before they are kept")
        } else {
            "no checker is configured for this file type: a `try_*` edit will be written \
             WITHOUT verification.  Prefer ast_rewrite where a grammar exists, and run the \
             project's own test command afterwards."
                .to_string()
        }),
    );
    Dynamic::from(map)
}

/// Which external tools this machine actually has, so an agent can plan
/// around what is missing instead of discovering it from a failure.
fn toolchain_capabilities() -> Dynamic {
    let probe = |bin: &str, arg: &str| -> Option<String> {
        Command::new(bin)
            .arg(arg)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                let text = String::from_utf8_lossy(&o.stdout);
                text.lines().next().unwrap_or("").trim().to_string()
            })
    };

    let tools = [
        ("cargo", "--version", "rustup component add cargo"),
        ("rustc", "--version", "rustup toolchain install stable"),
        ("ast-grep", "--version", "cargo install ast-grep --locked"),
        (
            "python3",
            "--version",
            "install python3 from your package manager",
        ),
        ("git", "--version", "install git from your package manager"),
    ];

    let mut map = Map::new();
    let mut missing = Array::new();
    for (bin, arg, hint) in tools {
        let mut entry = Map::new();
        match probe(bin, arg) {
            Some(v) => {
                entry.insert("installed".into(), Dynamic::from(true));
                entry.insert("version".into(), Dynamic::from(v));
                entry.insert("install_hint".into(), Dynamic::from(String::new()));
            }
            None => {
                entry.insert("installed".into(), Dynamic::from(false));
                entry.insert("version".into(), Dynamic::from(String::new()));
                entry.insert("install_hint".into(), Dynamic::from(hint.to_string()));
                missing.push(Dynamic::from(bin.to_string()));
            }
        }
        map.insert(bin.into(), Dynamic::from(entry));
    }
    map.insert("missing".into(), Dynamic::from(missing));
    Dynamic::from(map)
}

fn is_inside_cargo_workspace(path: &Path) -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    abs_path.starts_with(&cwd)
        && (cwd.join("Cargo.toml").exists() || abs_path.join("Cargo.toml").exists())
}

/// Push a file's modification time far enough forward that `cargo` cannot
/// mistake it for the build it already has.
///
/// Cargo decides a source is fresh when its mtime is not newer than the
/// output built from it.  A candidate written moments after the previous
/// check lands inside the same coarse timestamp tick, so cargo replays a
/// cached verdict about the *old* bytes — and every `try_*` editor keeps
/// its edit when that verdict says `ok`.  The guard is then silently
/// unsound exactly when an agent edits quickly, which is the normal case.
///
/// Two seconds is enough to clear one-second filesystem granularity, and
/// small enough that nothing else notices.
fn bump_mtime(target: &Path) {
    if !target.is_file() {
        return;
    }
    let future = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(target) {
        let _ = file.set_modified(future);
    }
}

/// Every file this process has written.
///
/// Bumping only the file being checked is not enough: `cargo check`
/// compiles the whole crate, so a *different* file edited moments earlier
/// in the same script is still stale and its errors are replayed from
/// cache — or worse, not replayed at all. Anything we have touched has to
/// look new.
static WRITTEN: std::sync::RwLock<Vec<PathBuf>> = std::sync::RwLock::new(Vec::new());

/// Note that `path` has been written, so the next check cannot miss it.
pub(crate) fn note_written(path: &Path) {
    if let Ok(mut guard) = WRITTEN.write() {
        let p = path.to_path_buf();
        if !guard.contains(&p) {
            guard.push(p);
        }
    }
}

fn bump_everything_written() {
    if let Ok(guard) = WRITTEN.read() {
        for path in guard.iter() {
            bump_mtime(path);
        }
    }
}

pub(crate) fn run_diagnostics(target: &Path) -> DiagnosticReport {
    // Before asking cargo anything, make sure it will actually look —
    // at this file and at everything else we have written.
    bump_mtime(target);
    bump_everything_written();

    let ext = target.extension().and_then(|s| s.to_str()).unwrap_or("");

    // 1. Rust file or workspace
    if ext == "rs" || target.join("Cargo.toml").exists() || target == Path::new(".") {
        if is_inside_cargo_workspace(target) {
            let output = Command::new("cargo")
                .arg("check")
                .arg("--message-format=json")
                .arg("--quiet")
                .output();

            if let Ok(out) = output {
                let stdout = String::from_utf8_lossy(&out.stdout);
                let mut errors = Vec::new();
                let mut warnings = Vec::new();

                for line in stdout.lines() {
                    if let Ok(val) = serde_json::from_str::<Value>(line) {
                        if val.get("reason").and_then(|r| r.as_str()) == Some("compiler-message") {
                            if let Some(msg) = val.get("message") {
                                let level =
                                    msg.get("level").and_then(|l| l.as_str()).unwrap_or("error");
                                let rendered =
                                    msg.get("rendered").and_then(|r| r.as_str()).unwrap_or("");
                                let spans = msg.get("spans").and_then(|s| s.as_array());

                                let (file, line_num, col) = if let Some(spans) = spans {
                                    if let Some(first) = spans.first() {
                                        (
                                            first
                                                .get("file_name")
                                                .and_then(|f| f.as_str())
                                                .unwrap_or("")
                                                .to_string(),
                                            first
                                                .get("line_start")
                                                .and_then(|l| l.as_u64())
                                                .unwrap_or(1)
                                                as u32,
                                            first
                                                .get("column_start")
                                                .and_then(|c| c.as_u64())
                                                .unwrap_or(1)
                                                as u32,
                                        )
                                    } else {
                                        (String::new(), 1, 1)
                                    }
                                } else {
                                    (String::new(), 1, 1)
                                };

                                let diag = Diagnostic {
                                    file,
                                    line: line_num,
                                    column: col,
                                    severity: level.to_string(),
                                    message: rendered.trim().to_string(),
                                };

                                if level == "error" {
                                    errors.push(diag);
                                } else if level == "warning" {
                                    warnings.push(diag);
                                }
                            }
                        }
                    }
                }

                if !out.status.success() || !errors.is_empty() {
                    return DiagnosticReport {
                        ok: false,
                        errors,
                        warnings,
                    };
                }
                return DiagnosticReport {
                    ok: true,
                    errors,
                    warnings,
                };
            }
        } else if ext == "rs" {
            // Standalone Rust file
            let output = Command::new("rustc")
                .arg("--crate-type=lib")
                .arg("--emit=metadata")
                .arg("-o")
                .arg("/tmp/umoja_diag_check.rmeta")
                .arg(target)
                .output();

            if let Ok(out) = output {
                if !out.status.success() {
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    let diag = Diagnostic {
                        file: target.display().to_string(),
                        line: 1,
                        column: 1,
                        severity: "error".to_string(),
                        message: stderr.trim().to_string(),
                    };
                    return DiagnosticReport {
                        ok: false,
                        errors: vec![diag],
                        warnings: vec![],
                    };
                }
                return DiagnosticReport {
                    ok: true,
                    errors: vec![],
                    warnings: vec![],
                };
            }
        }
    } else if ext == "py" {
        // Python syntax check
        let output = Command::new("python3")
            .arg("-m")
            .arg("py_compile")
            .arg(target)
            .output();

        if let Ok(out) = output {
            if !out.status.success() {
                let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                let diag = Diagnostic {
                    file: target.display().to_string(),
                    line: 1,
                    column: 1,
                    severity: "error".to_string(),
                    message: stderr.trim().to_string(),
                };
                return DiagnosticReport {
                    ok: false,
                    errors: vec![diag],
                    warnings: vec![],
                };
            }
        }
    } else if ext == "json" {
        if let Ok(content) = std::fs::read_to_string(target) {
            if let Err(e) = serde_json::from_str::<Value>(&content) {
                let diag = Diagnostic {
                    file: target.display().to_string(),
                    line: e.line() as u32,
                    column: e.column() as u32,
                    severity: "error".to_string(),
                    message: format!("Invalid JSON: {e}"),
                };
                return DiagnosticReport {
                    ok: false,
                    errors: vec![diag],
                    warnings: vec![],
                };
            }
        }
    }

    DiagnosticReport {
        ok: true,
        errors: vec![],
        warnings: vec![],
    }
}

// -----------------------------------------------------------------------------
// Speculative Edit Transaction
// -----------------------------------------------------------------------------

fn try_speculative_replace_lines(path: &str, start: usize, end: usize, new_text: &str) -> Dynamic {
    let original = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            let mut map = Map::new();
            map.insert("ok".into(), Dynamic::from(false));
            map.insert(
                "error".into(),
                Dynamic::from(format!("Could not read file {path}: {e}")),
            );
            map.insert("errors".into(), Dynamic::from(Array::new()));
            return Dynamic::from(map);
        }
    };

    // 1. Apply edit speculatively
    let lines: Vec<&str> = original.lines().collect();
    let s = (start.max(1) - 1).min(lines.len());
    let e = end.min(lines.len()).max(s);

    let mut result = Vec::new();
    result.extend_from_slice(&lines[..s]);
    if !new_text.is_empty() {
        result.push(new_text);
    }
    if e < lines.len() {
        result.extend_from_slice(&lines[e..]);
    }

    let mut candidate = result.join("\n");
    if original.ends_with('\n') {
        candidate.push('\n');
    }

    // Write candidate temporarily
    if std::fs::write(path, &candidate).is_err() {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert(
            "error".into(),
            Dynamic::from(format!("Could not write candidate to {path}")),
        );
        map.insert("errors".into(), Dynamic::from(Array::new()));
        return Dynamic::from(map);
    }

    // 2. Validate with LSP / Compiler diagnostics
    let report = run_diagnostics(Path::new(path));

    if !report.ok {
        // Rollback immediately!
        let _ = std::fs::write(path, &original);
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("rejected".into(), Dynamic::from(true));
        map.insert(
            "reason".into(),
            Dynamic::from("LSP compiler check failed. File was restored."),
        );
        map.insert(
            "error".into(),
            Dynamic::from("LSP compiler check failed. File was restored."),
        );

        let mut err_arr = Array::new();
        for err in report.errors {
            let mut em = Map::new();
            em.insert("file".into(), Dynamic::from(err.file));
            em.insert("line".into(), Dynamic::from(err.line as i64));
            em.insert("severity".into(), Dynamic::from(err.severity));
            em.insert("message".into(), Dynamic::from(err.message));
            err_arr.push(Dynamic::from(em));
        }
        map.insert("errors".into(), Dynamic::from(err_arr));
        return Dynamic::from(map);
    }

    // 3. Clean! Keep changes
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("applied".into(), Dynamic::from(true));
    map.insert(
        "lines_replaced".into(),
        Dynamic::from(format!("{start}-{end}")),
    );
    note_guard_as("try_replace_lines", &mut map, Path::new(path));
    Dynamic::from(map)
}

fn try_speculative_edit(path: &str, old_text: &str, new_text: &str) -> Dynamic {
    let original = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            let mut map = Map::new();
            map.insert("ok".into(), Dynamic::from(false));
            map.insert(
                "error".into(),
                Dynamic::from(format!("Could not read file {path}: {e}")),
            );
            map.insert("errors".into(), Dynamic::from(Array::new()));
            return Dynamic::from(map);
        }
    };

    if !original.contains(old_text) {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert(
            "error".into(),
            Dynamic::from(format!("Target text not found in {path}")),
        );
        map.insert("errors".into(), Dynamic::from(Array::new()));
        return Dynamic::from(map);
    }

    let candidate = original.replacen(old_text, new_text, 1);
    if std::fs::write(path, &candidate).is_err() {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("error".into(), Dynamic::from("Failed writing candidate"));
        map.insert("errors".into(), Dynamic::from(Array::new()));
        return Dynamic::from(map);
    }

    let report = run_diagnostics(Path::new(path));
    if !report.ok {
        let _ = std::fs::write(path, &original);
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("rejected".into(), Dynamic::from(true));
        map.insert(
            "reason".into(),
            Dynamic::from("LSP compiler check failed. File was restored."),
        );
        map.insert(
            "error".into(),
            Dynamic::from("LSP compiler check failed. File was restored."),
        );
        return Dynamic::from(map);
    }

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("applied".into(), Dynamic::from(true));
    note_guard(&mut map, Path::new(path));
    Dynamic::from(map)
}

fn try_speculative_replace_fn(path: &str, fn_name: &str, new_fn_body: &str) -> Dynamic {
    let original = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            let mut map = Map::new();
            map.insert("ok".into(), Dynamic::from(false));
            map.insert(
                "error".into(),
                Dynamic::from(format!("Could not read file {path}: {e}")),
            );
            map.insert("errors".into(), Dynamic::from(Array::new()));
            return Dynamic::from(map);
        }
    };

    let lines: Vec<&str> = original.lines().collect();
    let mut start_idx = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if (trimmed.starts_with("fn ")
            || trimmed.starts_with("pub fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub async fn "))
            && (trimmed.contains(&format!("fn {fn_name}"))
                || trimmed.contains(&format!("fn {fn_name}("))
                || trimmed.contains(&format!("fn {fn_name}<")))
        {
            start_idx = Some(idx);
            break;
        }
    }

    let Some(s) = start_idx else {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert(
            "error".into(),
            Dynamic::from(format!("Function '{fn_name}' not found in {path}")),
        );
        map.insert("errors".into(), Dynamic::from(Array::new()));
        return Dynamic::from(map);
    };

    let mut brace_depth = 0i32;
    let mut found_open = false;
    let mut end_idx = s;

    for (idx, line) in lines[s..].iter().enumerate() {
        let cur_idx = s + idx;
        for c in line.chars() {
            if c == '{' {
                brace_depth += 1;
                found_open = true;
            } else if c == '}' {
                brace_depth -= 1;
            }
        }
        if found_open && brace_depth <= 0 {
            end_idx = cur_idx;
            break;
        }
    }

    try_speculative_replace_lines(path, s + 1, end_idx + 1, new_fn_body)
}

// -----------------------------------------------------------------------------
// Module Creation Helper
// -----------------------------------------------------------------------------

fn create_new_module(path: &str, content: &str, parent_mod_file: Option<&str>) -> Dynamic {
    let file_path = Path::new(path);
    if let Some(parent) = file_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // 1. Write the new file
    if let Err(e) = std::fs::write(file_path, content) {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert(
            "error".into(),
            Dynamic::from(format!("Failed creating file {path}: {e}")),
        );
        map.insert("errors".into(), Dynamic::from(Array::new()));
        return Dynamic::from(map);
    }

    // 2. Link into parent module if Rust
    let mod_name = file_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let is_rust = file_path.extension().and_then(|s| s.to_str()) == Some("rs");

    if is_rust
        && !mod_name.is_empty()
        && mod_name != "mod"
        && mod_name != "lib"
        && mod_name != "main"
    {
        let parent_target = if let Some(p) = parent_mod_file {
            PathBuf::from(p)
        } else {
            // Auto-detect parent mod.rs or lib.rs in the directory
            let parent_dir = file_path.parent().unwrap_or(Path::new("."));
            let mod_rs = parent_dir.join("mod.rs");
            let lib_rs = parent_dir.join("lib.rs");
            let main_rs = parent_dir.join("main.rs");

            if mod_rs.exists() {
                mod_rs
            } else if lib_rs.exists() {
                lib_rs
            } else if main_rs.exists() {
                main_rs
            } else if let Some(grandparent) = parent_dir.parent() {
                let gp_mod = grandparent.join("mod.rs");
                let gp_lib = grandparent.join("lib.rs");
                if gp_mod.exists() {
                    gp_mod
                } else if gp_lib.exists() {
                    gp_lib
                } else {
                    parent_dir.join("mod.rs")
                }
            } else {
                parent_dir.join("mod.rs")
            }
        };

        let mod_decl = format!("pub mod {mod_name};");
        if parent_target.exists() {
            if let Ok(parent_content) = std::fs::read_to_string(&parent_target) {
                if !parent_content.contains(&format!("mod {mod_name};")) {
                    use std::io::Write;
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .append(true)
                        .open(&parent_target)
                    {
                        let _ = writeln!(f, "{mod_decl}");
                    }
                }
            }
        }
    }

    // 3. Run validation check
    let report = run_diagnostics(file_path);
    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(report.ok));
    map.insert("created".into(), Dynamic::from(path.to_string()));
    map.insert("valid".into(), Dynamic::from(report.ok));

    if !report.ok {
        let mut err_arr = Array::new();
        for err in report.errors {
            let mut em = Map::new();
            em.insert("file".into(), Dynamic::from(err.file));
            em.insert("line".into(), Dynamic::from(err.line as i64));
            em.insert("message".into(), Dynamic::from(err.message));
            err_arr.push(Dynamic::from(em));
        }
        map.insert("compiler_errors".into(), Dynamic::from(err_arr));
    }

    note_guard_as("create_module", &mut map, file_path);
    Dynamic::from(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The dangerous case: a file type nothing can check.  A guarded
    /// edit still writes it, so the result has to say so plainly rather
    /// than reporting a success it did not earn.
    #[test]
    fn an_unverifiable_file_type_is_reported_as_unguarded() {
        assert_eq!(checker_for(Path::new("a.ts")), "none");
        assert_eq!(checker_for(Path::new("a.go")), "none");
        assert_eq!(checker_for(Path::new("a.py")), "python3");
        assert_eq!(checker_for(Path::new("a.json")), "json");

        let mut map = Map::new();
        note_guard(&mut map, Path::new("a.ts"));
        assert_eq!(map.get("guarded").unwrap().clone().cast::<bool>(), false);
        assert!(map.contains_key("warning"));

        let mut checked = Map::new();
        note_guard(&mut checked, Path::new("a.py"));
        assert_eq!(checked.get("guarded").unwrap().clone().cast::<bool>(), true);
        assert!(
            !checked.contains_key("warning"),
            "a file that really is checked carries no warning"
        );
    }

    /// An agent must be able to ask what this machine has before it
    /// plans around a tool that is not there.
    #[test]
    fn capabilities_reports_every_probed_tool() {
        let mut engine = Engine::new();
        register_lsp_builtins(&mut engine);
        for tool in ["cargo", "rustc", "ast-grep", "python3", "git"] {
            let script = format!("capabilities()[\"{tool}\"].installed");
            engine
                .eval::<bool>(&script)
                .unwrap_or_else(|e| panic!("{tool} not reported: {e}"));
        }
        let missing = engine
            .eval::<rhai::Array>("capabilities().missing")
            .unwrap();
        let _ = missing.len();
    }

    /// The mechanism behind the stale-verdict fix: after a write, the file
    /// must look strictly newer than anything built from it, or cargo
    /// answers about the previous contents.
    #[test]
    fn a_checked_file_is_made_newer_than_any_build_of_it() {
        let dir = std::env::temp_dir().join("umoja_bump_mtime");
        let _ = std::fs::create_dir_all(&dir);
        let file = dir.join("candidate.rs");
        std::fs::write(&file, "fn a() {}\n").unwrap();

        let before = std::fs::metadata(&file).unwrap().modified().unwrap();
        bump_mtime(&file);
        let after = std::fs::metadata(&file).unwrap().modified().unwrap();

        assert!(
            after > before,
            "the candidate must be newer than it was, or cargo replays a cached verdict"
        );
        assert!(
            after > std::time::SystemTime::now(),
            "it must also be newer than the build that is about to read it"
        );
    }

    /// A directory is a legitimate target (`lsp_diagnostics` passes `.`),
    /// and must not be disturbed.
    #[test]
    fn bumping_a_directory_is_a_no_op() {
        let dir = std::env::temp_dir().join("umoja_bump_dir");
        let _ = std::fs::create_dir_all(&dir);
        let before = std::fs::metadata(&dir).unwrap().modified().unwrap();
        bump_mtime(&dir);
        let after = std::fs::metadata(&dir).unwrap().modified().unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn test_diagnostics_report() {
        let report = run_diagnostics(Path::new("Cargo.toml"));
        assert!(report.ok);
    }
}
