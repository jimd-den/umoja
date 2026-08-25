//! Structural, syntax-aware search and rewrite backed by bundled `ast-grep-core`.
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

use ast_grep_core::AstGrep;
use ast_grep_language::SupportLang;
use rhai::{Array, Dynamic, Engine, Map};
use serde_json::Value;

use super::lsp::run_diagnostics;

/// The binary that answers for structural queries if external execution is preferred.
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

    // High-productivity structural shortcuts
    engine.register_fn("ast_find_fn", |name: &str| -> Dynamic {
        ast_find_function(name, "**/*.rs")
    });
    engine.register_fn("ast_find_fn", |name: &str, glob: &str| -> Dynamic {
        ast_find_function(name, glob)
    });

    engine.register_fn("ast_find_calls", |name: &str| -> Dynamic {
        ast_find_callsites(name, "**/*.rs")
    });
    engine.register_fn("ast_find_calls", |name: &str, glob: &str| -> Dynamic {
        ast_find_callsites(name, glob)
    });

    engine.register_fn("ast_find_struct", |name: &str| -> Dynamic {
        ast_find_structures(name, "**/*.rs")
    });
    engine.register_fn("ast_find_struct", |name: &str, glob: &str| -> Dynamic {
        ast_find_structures(name, glob)
    });

    engine.register_fn("validate_syntax", |code: &str| -> Dynamic {
        validate_code_syntax(code, "rust")
    });
    engine.register_fn("validate_syntax", |code: &str, lang: &str| -> Dynamic {
        validate_code_syntax(code, lang)
    });

    // -------------------------------------------------------------------------
    // Symbol-Level Code Reading Inverses: read_fn, read_impl, enclosing
    // -------------------------------------------------------------------------
    engine.register_fn("read_fn", |path: &str, name: &str| -> Dynamic {
        read_symbol_function(path, name)
    });

    engine.register_fn("read_impl", |path: &str, type_name: &str| -> Dynamic {
        read_symbol_impl(path, type_name)
    });

    engine.register_fn("enclosing", |path: &str, line: i64| -> Dynamic {
        find_enclosing_symbol(path, line as usize)
    });
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

    // Resolve glob paths or directories
    let mut target_paths: Vec<String> = Vec::new();
    let has_glob = path.contains(42 as char) || path.contains(63 as char) || path.contains(91 as char);

    if has_glob {
        let _root_dir = if path.starts_with("crates/") || path.starts_with("crates\\") {
            "."
        } else if let Some(manifest) = std::env::var("CARGO_MANIFEST_DIR").ok() {
            // In unit tests under cargo, cwd may be the subcrate directory
            let parent = Path::new(&manifest).parent().and_then(|p| p.parent()).map(|p| p.to_path_buf());
            if let Some(p) = parent {
                if p.join(path).exists() || p.join("crates").exists() {
                    // Search from workspace root
                }
            }
            "."
        } else {
            "."
        };

        if let Ok(entries) = glob::glob(path) {
            for entry in entries.flatten() {
                target_paths.push(entry.display().to_string());
            }
        }

        // Also attempt glob from parent workspace directory if run from subcrate
        if target_paths.is_empty() {
            let parent_pattern = format!("../../{path}");
            if let Ok(entries) = glob::glob(&parent_pattern) {
                for entry in entries.flatten() {
                    target_paths.push(entry.display().to_string());
                }
            }
        }
    } else {
        target_paths.push(path.to_string());
    }

    if target_paths.is_empty() {
        target_paths.push(path.to_string());
    }

    let mut parsed: Vec<Value> = Vec::new();

    // Chunk paths to avoid exceeding CLI argument limits
    for chunk in target_paths.chunks(100) {
        let mut cmd = Command::new(AST_GREP);
        cmd.arg("run")
            .arg("--pattern")
            .arg(pattern)
            .arg("--json=compact");
        with_lang(&mut cmd, lang);
        for target in chunk {
            cmd.arg(target);
        }

        if let Ok(out) = cmd.output() {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if let Ok(items) = serde_json::from_str::<Vec<Value>>(stdout.trim()) {
                parsed.extend(items);
            }
        }
    }

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
        let lines_val = hit.get("lines").and_then(|v| v.as_str()).unwrap_or("").to_string();
        m.insert("context".into(), Dynamic::from(lines_val.clone()));
        m.insert("lines".into(), Dynamic::from(lines_val));
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
    super::lsp::note_guard_as("ast_rewrite", &mut map, Path::new(path));
    Dynamic::from(map)
}

fn parse_support_lang(lang: &str) -> SupportLang {
    match lang.to_ascii_lowercase().as_str() {
        "rs" | "rust" => SupportLang::Rust,
        "ts" | "typescript" => SupportLang::TypeScript,
        "tsx" => SupportLang::Tsx,
        "js" | "javascript" => SupportLang::JavaScript,
        "py" | "python" => SupportLang::Python,
        "go" | "golang" => SupportLang::Go,
        "c" => SupportLang::C,
        "cpp" | "c++" | "cxx" => SupportLang::Cpp,
        "cs" | "csharp" | "c#" => SupportLang::CSharp,
        "java" => SupportLang::Java,
        "json" => SupportLang::Json,
        "yaml" | "yml" => SupportLang::Yaml,
        "html" => SupportLang::Html,
        "css" => SupportLang::Css,
        "bash" | "sh" => SupportLang::Bash,
        "lua" => SupportLang::Lua,
        _ => SupportLang::Rust,
    }
}

/// In-memory syntax validation for generated code before writing to disk.
pub(crate) fn validate_code_syntax(code: &str, lang_str: &str) -> Dynamic {
    let lang = parse_support_lang(lang_str);
    let mut map = Map::new();

    // 1. Bracket & delimiter matching check
    let mut stack = Vec::new();
    for (line_idx, line) in code.lines().enumerate() {
        let line_num = line_idx + 1;
        for (col_idx, ch) in line.chars().enumerate() {
            let col_num = col_idx + 1;
            if ch == 40 as char || ch == 123 as char || ch == 91 as char {
                stack.push((ch, line_num, col_num));
            } else if ch == 41 as char {
                if stack.pop().map(|(open, _, _)| open != 40 as char).unwrap_or(true) {
                    map.insert("ok".into(), Dynamic::from(false));
                    map.insert("error".into(), Dynamic::from(format!("Unmatched closing parenthesis at line {line_num}:{col_num}")));
                    map.insert("line".into(), Dynamic::from(line_num as i64));
                    return Dynamic::from(map);
                }
            } else if ch == 125 as char {
                if stack.pop().map(|(open, _, _)| open != 123 as char).unwrap_or(true) {
                    map.insert("ok".into(), Dynamic::from(false));
                    map.insert("error".into(), Dynamic::from(format!("Unmatched closing brace at line {line_num}:{col_num}")));
                    map.insert("line".into(), Dynamic::from(line_num as i64));
                    return Dynamic::from(map);
                }
            } else if ch == 93 as char {
                if stack.pop().map(|(open, _, _)| open != 91 as char).unwrap_or(true) {
                    map.insert("ok".into(), Dynamic::from(false));
                    map.insert("error".into(), Dynamic::from(format!("Unmatched closing bracket at line {line_num}:{col_num}")));
                    map.insert("line".into(), Dynamic::from(line_num as i64));
                    return Dynamic::from(map);
                }
            }
        }
    }

    if let Some((unclosed, line_num, col_num)) = stack.pop() {
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("error".into(), Dynamic::from(format!("Unclosed delimiter {unclosed} opened at line {line_num}:{col_num}")));
        map.insert("line".into(), Dynamic::from(line_num as i64));
        return Dynamic::from(map);
    }

    // 2. Tree-sitter AST parse
    let root = AstGrep::new(code, lang);
    let root_node = root.root();
    if root_node.is_error() {
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("error".into(), Dynamic::from("Syntax error detected in AST parse tree".to_string()));
        return Dynamic::from(map);
    }

    map.insert("ok".into(), Dynamic::from(true));
    map.insert("valid".into(), Dynamic::from(true));
    Dynamic::from(map)
}

fn ast_find_function(name: &str, glob_pattern: &str) -> Dynamic {
    let pattern = if name.is_empty() || name == "*" {
        "fn $NAME($$$ARGS) -> $RET { $$$BODY }".to_string()
    } else {
        format!("fn {name}($$$ARGS) -> $RET {{ $$$BODY }}")
    };
    search(&pattern, glob_pattern, Some("rust"))
}

fn ast_find_callsites(name: &str, glob_pattern: &str) -> Dynamic {
    let pattern = format!("{name}($$$ARGS)");
    search(&pattern, glob_pattern, Some("rust"))
}

fn ast_find_structures(name: &str, glob_pattern: &str) -> Dynamic {
    let pattern = if name.is_empty() || name == "*" {
        "struct $NAME { $$$FIELDS }".to_string()
    } else {
        format!("struct {name} {{ $$$FIELDS }}")
    };
    search(&pattern, glob_pattern, Some("rust"))
}

/// Reads the exact body, signature, line numbers, and doc comments of a function.
fn read_symbol_function(path: &str, name: &str) -> Dynamic {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return error_map(&format!("could not read {path}: {e}")),
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut start_line = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if (trimmed.starts_with("fn ")
            || trimmed.starts_with("pub fn ")
            || trimmed.starts_with("pub(crate) fn ")
            || trimmed.starts_with("async fn ")
            || trimmed.starts_with("pub async fn ")
            || trimmed.starts_with("pub(crate) async fn "))
            && (trimmed.contains(&format!("fn {name}"))
                || trimmed.contains(&format!("fn {name}("))
                || trimmed.contains(&format!("fn {name}<")))
        {
            start_line = Some(idx);
            break;
        }
    }

    let Some(start_idx) = start_line else {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("error".into(), Dynamic::from(format!("Function {name} not found in {path}")));
        return Dynamic::from(map);
    };

    // Scan backwards for doc comments or attributes
    let mut doc_start_idx = start_idx;
    while doc_start_idx > 0 {
        let prev = lines[doc_start_idx - 1].trim();
        if prev.starts_with("///") || prev.starts_with("//!") || prev.starts_with("#[") {
            doc_start_idx -= 1;
        } else {
            break;
        }
    }

    let mut brace_depth = 0i32;
    let mut found_open = false;
    let mut end_idx = start_idx;

    for (idx, line) in lines[start_idx..].iter().enumerate() {
        let actual_idx = start_idx + idx;
        for ch in line.chars() {
            if ch == 123 as char {
                brace_depth += 1;
                found_open = true;
            } else if ch == 125 as char {
                brace_depth -= 1;
            }
        }
        if found_open && brace_depth <= 0 {
            end_idx = actual_idx;
            break;
        }
    }

    let fn_lines = &lines[doc_start_idx..=end_idx];
    let body_text = fn_lines.join(std::str::from_utf8(&[10]).unwrap());

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("name".into(), Dynamic::from(name.to_string()));
    map.insert("file".into(), Dynamic::from(path.to_string()));
    map.insert("start_line".into(), Dynamic::from((doc_start_idx + 1) as i64));
    map.insert("end_line".into(), Dynamic::from((end_idx + 1) as i64));
    map.insert("line_count".into(), Dynamic::from(fn_lines.len() as i64));
    map.insert("body".into(), Dynamic::from(body_text.clone()));
    map.insert("text".into(), Dynamic::from(body_text));
    Dynamic::from(map)
}

/// Reads the entire `impl` block for a given type name.
fn read_symbol_impl(path: &str, type_name: &str) -> Dynamic {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return error_map(&format!("could not read {path}: {e}")),
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut start_line = None;

    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("impl ")
            && (trimmed.contains(&format!(" {type_name} "))
                || trimmed.contains(&format!(" {type_name}<"))
                || trimmed.contains(&format!(" {type_name}{{"))
                || trimmed.ends_with(&format!(" {type_name}"))
                || trimmed.ends_with(&format!(" {type_name} {{")))
        {
            start_line = Some(idx);
            break;
        }
    }

    let Some(start_idx) = start_line else {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("error".into(), Dynamic::from(format!("Impl for {type_name} not found in {path}")));
        return Dynamic::from(map);
    };

    let mut brace_depth = 0i32;
    let mut found_open = false;
    let mut end_idx = start_idx;

    for (idx, line) in lines[start_idx..].iter().enumerate() {
        let actual_idx = start_idx + idx;
        for ch in line.chars() {
            if ch == 123 as char {
                brace_depth += 1;
                found_open = true;
            } else if ch == 125 as char {
                brace_depth -= 1;
            }
        }
        if found_open && brace_depth <= 0 {
            end_idx = actual_idx;
            break;
        }
    }

    let impl_lines = &lines[start_idx..=end_idx];
    let body_text = impl_lines.join(std::str::from_utf8(&[10]).unwrap());

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("type".into(), Dynamic::from(type_name.to_string()));
    map.insert("file".into(), Dynamic::from(path.to_string()));
    map.insert("start_line".into(), Dynamic::from((start_idx + 1) as i64));
    map.insert("end_line".into(), Dynamic::from((end_idx + 1) as i64));
    map.insert("line_count".into(), Dynamic::from(impl_lines.len() as i64));
    map.insert("body".into(), Dynamic::from(body_text.clone()));
    map.insert("text".into(), Dynamic::from(body_text));
    Dynamic::from(map)
}

/// Identifies the enclosing function, struct, or impl block containing a line number.
fn find_enclosing_symbol(path: &str, target_line: usize) -> Dynamic {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return error_map(&format!("could not read {path}: {e}")),
    };

    let target_idx = target_line.saturating_sub(1);
    let lines: Vec<&str> = content.lines().collect();

    if target_idx >= lines.len() {
        let mut map = Map::new();
        map.insert("ok".into(), Dynamic::from(false));
        map.insert("error".into(), Dynamic::from(format!("Line {target_line} is outside {path} (total lines: {})", lines.len())));
        return Dynamic::from(map);
    }

    // Scan upward to find enclosing function or block header
    let mut current_fn = None;
    let mut current_impl = None;
    let mut current_struct = None;

    let mut brace_depth = 0i32;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ") || trimmed.starts_with("pub(crate) fn ") || trimmed.starts_with("async fn ") || trimmed.starts_with("pub async fn ") {
            current_fn = Some((trimmed.to_string(), idx + 1));
        } else if trimmed.starts_with("impl ") {
            current_impl = Some((trimmed.to_string(), idx + 1));
        } else if trimmed.starts_with("struct ") || trimmed.starts_with("pub struct ") || trimmed.starts_with("enum ") || trimmed.starts_with("pub enum ") {
            current_struct = Some((trimmed.to_string(), idx + 1));
        }

        for ch in line.chars() {
            if ch == 123 as char {
                brace_depth += 1;
            } else if ch == 125 as char {
                brace_depth -= 1;
                if brace_depth <= 0 && idx < target_idx {
                    current_fn = None;
                }
            }
        }

        if idx == target_idx {
            break;
        }
    }

    let mut map = Map::new();
    map.insert("ok".into(), Dynamic::from(true));
    map.insert("file".into(), Dynamic::from(path.to_string()));
    map.insert("line".into(), Dynamic::from(target_line as i64));

    if let Some((fn_sig, fn_line)) = current_fn {
        map.insert("kind".into(), Dynamic::from("function".to_string()));
        map.insert("name".into(), Dynamic::from(fn_sig));
        map.insert("start_line".into(), Dynamic::from(fn_line as i64));
    } else if let Some((impl_sig, impl_line)) = current_impl {
        map.insert("kind".into(), Dynamic::from("impl".to_string()));
        map.insert("name".into(), Dynamic::from(impl_sig));
        map.insert("start_line".into(), Dynamic::from(impl_line as i64));
    } else if let Some((st_sig, st_line)) = current_struct {
        map.insert("kind".into(), Dynamic::from("struct_or_enum".to_string()));
        map.insert("name".into(), Dynamic::from(st_sig));
        map.insert("start_line".into(), Dynamic::from(st_line as i64));
    } else {
        map.insert("kind".into(), Dynamic::from("top_level".to_string()));
        map.insert("name".into(), Dynamic::from(String::new()));
        map.insert("start_line".into(), Dynamic::from(1i64));
    }

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

    #[test]
    fn test_validate_syntax_pre_flight_checks() {
        let mut engine = Engine::new();
        register_astgrep_builtins(&mut engine);

        // 1. Valid Rust code passes
        let valid_res: Map = engine.eval(r#"validate_syntax("fn foo() { let x = 1; }", "rust")"#).unwrap();
        assert_eq!(valid_res.get("ok").unwrap().clone().cast::<bool>(), true);

        // 2. Unclosed delimiter rejected instantly
        let unclosed_res: Map = engine.eval(r#"validate_syntax("fn foo() { let x = (1 + 2;", "rust")"#).unwrap();
        assert_eq!(unclosed_res.get("ok").unwrap().clone().cast::<bool>(), false);
        assert!(unclosed_res.get("error").unwrap().clone().into_string().unwrap().contains("Unclosed"));

        // 3. Unmatched closing delimiter rejected instantly
        let unmatched_res: Map = engine.eval(r#"validate_syntax("fn foo() { let x = 1; } }", "rust")"#).unwrap();
        assert_eq!(unmatched_res.get("ok").unwrap().clone().cast::<bool>(), false);
        assert!(unmatched_res.get("error").unwrap().clone().into_string().unwrap().contains("Unmatched"));

        // 4. Glob expansion in ast_grep
        let glob_res: Map = engine.eval(r#"ast_grep("$X.insert($A)", "crates/**/*.rs", "rust")"#).unwrap();
        assert!(glob_res.get("count").unwrap().clone().cast::<i64>() > 0);
        let first_match = glob_res.get("matches").unwrap().clone().cast::<Array>()[0].clone().cast::<Map>();
        assert!(first_match.contains_key("context"));

        // 5. read_fn, read_impl, enclosing
        let temp_reader_file = "/tmp/umoja_test_reader.rs";
        let code = "/// Doc comment\npub fn calculate_area(w: i32, h: i32) -> i32 {\n    w * h\n}\n\nstruct Rect;\nimpl Rect {\n    fn new() -> Self { Rect }\n}\n";
        std::fs::write(temp_reader_file, code).unwrap();

        let fn_res: Map = engine.eval(&format!(r#"read_fn("{temp_reader_file}", "calculate_area")"#)).unwrap();
        assert_eq!(fn_res.get("ok").unwrap().clone().cast::<bool>(), true);
        assert_eq!(fn_res.get("start_line").unwrap().clone().cast::<i64>(), 1);
        assert!(fn_res.get("body").unwrap().clone().into_string().unwrap().contains("w * h"));

        let impl_res: Map = engine.eval(&format!(r#"read_impl("{temp_reader_file}", "Rect")"#)).unwrap();
        assert_eq!(impl_res.get("ok").unwrap().clone().cast::<bool>(), true);
        assert!(impl_res.get("body").unwrap().clone().into_string().unwrap().contains("fn new"));

        let enc_res: Map = engine.eval(&format!(r#"enclosing("{temp_reader_file}", 3)"#)).unwrap();
        assert_eq!(enc_res.get("ok").unwrap().clone().cast::<bool>(), true);
        assert_eq!(enc_res.get("kind").unwrap().clone().into_string().unwrap(), "function");

        let _ = std::fs::remove_file(temp_reader_file);
    }
}
