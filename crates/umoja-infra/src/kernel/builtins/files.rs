//! File inspection, editing, multi-file loading, and search operations for Rhai.

use std::path::Path;
use std::process::Command;

use base64::prelude::*;
use base64::Engine as _;
use glob::glob;
use rhai::{Array, Dynamic, Engine, Map};
use serde_json::Value;

/// Record a write that nothing verified.
///
/// These are the mutations worth counting: `write`, `edit` and the
/// line-range family put bytes on disk with no checker in the loop, so the
/// journal is the only evidence they happened.
fn note_unguarded(op: &str, path: &str) {
    crate::activity::record_mutation(op, path, false, "none");
    super::lsp::note_written(Path::new(path));
}

pub fn register_files_builtins(engine: &mut Engine) {
    // -------------------------------------------------------------------------
    // head & tail
    // -------------------------------------------------------------------------
    engine.register_fn("head", |path: &str| -> String {
        head_lines(path, 50)
    });
    engine.register_fn("head", |path: &str, lines: i64| -> String {
        head_lines(path, lines.max(0) as usize)
    });

    engine.register_fn("tail", |path: &str| -> String {
        tail_lines(path, 50)
    });
    engine.register_fn("tail", |path: &str, lines: i64| -> String {
        tail_lines(path, lines.max(0) as usize)
    });

    // -------------------------------------------------------------------------
    // slice_lines & outline
    // -------------------------------------------------------------------------
    engine.register_fn("slice_lines", |path: &str, start: i64, end: i64| -> String {
        slice_file_lines(path, start.max(1) as usize, end.max(1) as usize)
    });

    engine.register_fn("outline", |path: &str| -> String {
        outline_file(path)
    });

    // -------------------------------------------------------------------------
    // read, write, edit
    // -------------------------------------------------------------------------
    engine.register_fn("read", |path: &str| -> String {
        std::fs::read_to_string(path).unwrap_or_else(|e| format!("Error reading {path}: {e}"))
    });

    engine.register_fn("write", |path: &str, content: &str| -> bool {
        if let Some(parent) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let ok = std::fs::write(path, content).is_ok();
        if ok {
            note_unguarded("write", path);
        }
        ok
    });

    engine.register_fn("edit", |path: &str, old_text: &str, new_text: &str| -> bool {
        if let Ok(content) = std::fs::read_to_string(path) {
            if content.contains(old_text) {
                let replaced = content.replacen(old_text, new_text, 1);
                let ok = std::fs::write(path, replaced).is_ok();
                if ok {
                    note_unguarded("edit", path);
                }
                return ok;
            }
        }
        false
    });

    // -------------------------------------------------------------------------
    // Dedicated Line-Range & Position Editing (Immune to quote escaping)
    // -------------------------------------------------------------------------
    engine.register_fn("replace_lines", |path: &str, start: i64, end: i64, new_text: &str| -> bool {
        let ok = replace_lines_in_file(path, start.max(1) as usize, end.max(1) as usize, new_text);
        if ok {
            note_unguarded("replace_lines", path);
        }
        ok
    });

    engine.register_fn("insert_at_line", |path: &str, line_num: i64, text: &str| -> bool {
        insert_line_at(path, line_num.max(1) as usize, text)
    });

    engine.register_fn("insert_after", |path: &str, anchor: &str, text: &str| -> bool {
        insert_relative_to_anchor(path, anchor, text, true)
    });

    engine.register_fn("insert_before", |path: &str, anchor: &str, text: &str| -> bool {
        insert_relative_to_anchor(path, anchor, text, false)
    });

    engine.register_fn("append_lines", |path: &str, text: &str| -> bool {
        if let Some(parent) = Path::new(path).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        use std::io::Write;
        if let Ok(mut file) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(file, "{text}");
            true
        } else {
            false
        }
    });

    // -------------------------------------------------------------------------
    // Symbol & Function-Level AST Edits
    // -------------------------------------------------------------------------
    engine.register_fn("replace_fn", |path: &str, fn_name: &str, new_fn_body: &str| -> bool {
        replace_symbol_block(path, &["fn ", "def ", "function "], fn_name, new_fn_body)
    });

    engine.register_fn("replace_struct", |path: &str, struct_name: &str, new_body: &str| -> bool {
        replace_symbol_block(path, &["struct ", "class ", "interface ", "type "], struct_name, new_body)
    });

    engine.register_fn("replace_impl", |path: &str, type_name: &str, method_name: &str, new_method_body: &str| -> bool {
        replace_impl_method(path, type_name, method_name, new_method_body)
    });

    // -------------------------------------------------------------------------
    // Base64 & Safe Stream Ingestion
    // -------------------------------------------------------------------------
    engine.register_fn("write_b64", |path: &str, b64_payload: &str| -> bool {
        if let Ok(bytes) = BASE64_STANDARD.decode(b64_payload.trim()) {
            if let Some(parent) = Path::new(path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(path, bytes).is_ok()
        } else {
            false
        }
    });

    engine.register_fn("read_b64", |path: &str| -> String {
        if let Ok(bytes) = std::fs::read(path) {
            BASE64_STANDARD.encode(bytes)
        } else {
            String::new()
        }
    });

    engine.register_fn("edit_b64", |path: &str, old_b64: &str, new_b64: &str| -> bool {
        if let (Ok(old_bytes), Ok(new_bytes)) = (
            BASE64_STANDARD.decode(old_b64.trim()),
            BASE64_STANDARD.decode(new_b64.trim()),
        ) {
            if let (Ok(old_text), Ok(new_text)) = (
                String::from_utf8(old_bytes),
                String::from_utf8(new_bytes),
            ) {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if content.contains(&old_text) {
                        let replaced = content.replacen(&old_text, &new_text, 1);
                        return std::fs::write(path, replaced).is_ok();
                    }
                }
            }
        }
        false
    });

    engine.register_fn("replace_lines_b64", |path: &str, start: i64, end: i64, new_b64: &str| -> bool {
        if let Ok(new_bytes) = BASE64_STANDARD.decode(new_b64.trim()) {
            if let Ok(new_text) = String::from_utf8(new_bytes) {
                return replace_lines_in_file(path, start.max(1) as usize, end.max(1) as usize, &new_text);
            }
        }
        false
    });

    engine.register_fn("b64_encode", |text: &str| -> String {
        BASE64_STANDARD.encode(text.as_bytes())
    });

    engine.register_fn("b64_decode", |b64: &str| -> String {
        BASE64_STANDARD.decode(b64.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_default()
    });

    // -------------------------------------------------------------------------
    // load
    // -------------------------------------------------------------------------
    engine.register_fn("load", |pattern: &str| -> Dynamic {
        if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
            let mut results = Array::new();
            if let Ok(paths) = glob(pattern) {
                for entry in paths.flatten() {
                    if entry.is_file() && !is_ignored_path(&entry) {
                        let path_str = entry.display().to_string();
                        if let Ok(content) = std::fs::read_to_string(&entry) {
                            let line_count = content.lines().count() as i64;
                            let size = content.len() as i64;
                            let mut file_map = Map::new();
                            file_map.insert("path".into(), Dynamic::from(path_str));
                            file_map.insert("content".into(), Dynamic::from(content));
                            file_map.insert("lines".into(), Dynamic::from(line_count));
                            file_map.insert("size".into(), Dynamic::from(size));
                            results.push(Dynamic::from(file_map));
                        }
                    }
                }
            }
            Dynamic::from(results)
        } else {
            let path = Path::new(pattern);
            if path.extension().and_then(|s| s.to_str()) == Some("json") {
                if let Ok(content) = std::fs::read_to_string(path) {
                    if let Ok(val) = serde_json::from_str::<Value>(&content) {
                        if let Ok(dyn_val) = rhai::serde::to_dynamic(&val) {
                            return dyn_val;
                        }
                    }
                }
            }
            if let Ok(content) = std::fs::read_to_string(path) {
                Dynamic::from(content)
            } else {
                Dynamic::from(format!("Error: could not read file '{pattern}'"))
            }
        }
    });

    // -------------------------------------------------------------------------
    // grep
    // -------------------------------------------------------------------------
    engine.register_fn("grep", |pattern: &str| -> Dynamic {
        grep_path(".", pattern)
    });

    engine.register_fn("grep", |pattern: &str, target: &str| -> Dynamic {
        grep_path(target, pattern)
    });

    engine.register_fn("grep", |files: Array, pattern: &str| -> Array {
        grep_array(&files, pattern)
    });

    engine.register_fn("grep", |files: &mut Array, pattern: &str| -> Array {
        grep_array(files, pattern)
    });

    // -------------------------------------------------------------------------
    // sh
    // -------------------------------------------------------------------------
    engine.register_fn("sh", |cmd: &str| -> String {
        let output = Command::new("sh").arg("-c").arg(cmd).output();
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                if !stdout.is_empty() {
                    stdout.trim_end().to_string()
                } else {
                    String::from_utf8_lossy(&out.stderr).trim_end().to_string()
                }
            }
            Err(e) => format!("sh error: {e}"),
        }
    });

    // -------------------------------------------------------------------------
    // JSON parsing & stringification
    // -------------------------------------------------------------------------
    engine.register_fn("parse_json", |s: &str| -> Dynamic {
        serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| rhai::serde::to_dynamic(&v).ok())
            .unwrap_or(Dynamic::UNIT)
    });
    engine.register_fn("json_parse", |s: &str| -> Dynamic {
        serde_json::from_str::<Value>(s)
            .ok()
            .and_then(|v| rhai::serde::to_dynamic(&v).ok())
            .unwrap_or(Dynamic::UNIT)
    });

    engine.register_fn("to_json", |val: Dynamic| -> String {
        rhai::serde::from_dynamic::<Value>(&val)
            .ok()
            .and_then(|v| serde_json::to_string_pretty(&v).ok())
            .unwrap_or_else(|| val.to_string())
    });
    engine.register_fn("json_stringify", |val: Dynamic| -> String {
        rhai::serde::from_dynamic::<Value>(&val)
            .ok()
            .and_then(|v| serde_json::to_string(&v).ok())
            .unwrap_or_else(|| val.to_string())
    });
}

// -----------------------------------------------------------------------------
// Line-Range and Relative Editing Helpers
// -----------------------------------------------------------------------------

fn replace_lines_in_file(path: &str, start: usize, end: usize, new_text: &str) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = content.lines().collect();
        if lines.is_empty() && start == 1 {
            return std::fs::write(path, new_text).is_ok();
        }
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

        let mut output = result.join("\n");
        if content.ends_with('\n') {
            output.push('\n');
        }
        return std::fs::write(path, output).is_ok();
    }
    false
}

fn insert_line_at(path: &str, line_num: usize, text: &str) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = content.lines().collect();
        let idx = (line_num.max(1) - 1).min(lines.len());

        let mut result = Vec::new();
        result.extend_from_slice(&lines[..idx]);
        result.push(text);
        if idx < lines.len() {
            result.extend_from_slice(&lines[idx..]);
        }

        let mut output = result.join("\n");
        if content.ends_with('\n') {
            output.push('\n');
        }
        return std::fs::write(path, output).is_ok();
    }
    false
}

fn insert_relative_to_anchor(path: &str, anchor: &str, text: &str, after: bool) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if line.contains(anchor) {
                let target_idx = if after { idx + 1 } else { idx };
                let mut result = Vec::new();
                result.extend_from_slice(&lines[..target_idx]);
                result.push(text);
                if target_idx < lines.len() {
                    result.extend_from_slice(&lines[target_idx..]);
                }
                let mut output = result.join("\n");
                if content.ends_with('\n') {
                    output.push('\n');
                }
                return std::fs::write(path, output).is_ok();
            }
        }
    }
    false
}

// -----------------------------------------------------------------------------
// Symbol-Level Block Replacement Helpers
// -----------------------------------------------------------------------------

fn replace_symbol_block(path: &str, prefixes: &[&str], name: &str, new_body: &str) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = content.lines().collect();
        let mut start_idx = None;

        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            for prefix in prefixes {
                if trimmed.contains(prefix) && (trimmed.contains(&format!("{prefix}{name}")) || trimmed.contains(&format!("{prefix} {name}")) || trimmed.contains(&format!("{name}(")) || trimmed.contains(&format!("{name} ")) || trimmed.contains(&format!("{name}<"))) {
                    start_idx = Some(idx);
                    break;
                }
            }
            if start_idx.is_some() {
                break;
            }
        }

        if let Some(s) = start_idx {
            // Find balanced closing brace or next top-level item
            let mut brace_depth = 0i32;
            let mut found_open = false;
            let mut end_idx = s;

            for (idx, line) in lines[s..].iter().enumerate() {
                let actual_idx = s + idx;
                for c in line.chars() {
                    if c == '{' {
                        brace_depth += 1;
                        found_open = true;
                    } else if c == '}' {
                        brace_depth -= 1;
                    }
                }
                if found_open && brace_depth <= 0 {
                    end_idx = actual_idx;
                    break;
                }
            }

            return replace_lines_in_file(path, s + 1, end_idx + 1, new_body);
        }
    }
    false
}

fn replace_impl_method(path: &str, type_name: &str, method_name: &str, new_body: &str) -> bool {
    if let Ok(content) = std::fs::read_to_string(path) {
        let lines: Vec<&str> = content.lines().collect();
        let mut impl_start = None;

        for (idx, line) in lines.iter().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with("impl") && trimmed.contains(type_name) {
                impl_start = Some(idx);
                break;
            }
        }

        if let Some(start) = impl_start {
            for (idx, line) in lines[start..].iter().enumerate() {
                let actual_idx = start + idx;
                let trimmed = line.trim();
                if (trimmed.starts_with("fn ") || trimmed.starts_with("pub fn ")) && (trimmed.contains(&format!("fn {method_name}")) || trimmed.contains(&format!("fn {method_name}("))) {
                    let mut brace_depth = 0i32;
                    let mut found_open = false;
                    let mut method_end = actual_idx;

                    for (m_idx, m_line) in lines[actual_idx..].iter().enumerate() {
                        let cur_idx = actual_idx + m_idx;
                        for c in m_line.chars() {
                            if c == '{' {
                                brace_depth += 1;
                                found_open = true;
                            } else if c == '}' {
                                brace_depth -= 1;
                            }
                        }
                        if found_open && brace_depth <= 0 {
                            method_end = cur_idx;
                            break;
                        }
                    }

                    return replace_lines_in_file(path, actual_idx + 1, method_end + 1, new_body);
                }
            }
        }
    }
    false
}

fn is_ignored_path(path: &Path) -> bool {
    for comp in path.components() {
        if let std::path::Component::Normal(os_str) = comp {
            let s = os_str.to_string_lossy();
            if s == "target" || s == ".git" || s == "node_modules" || s == ".cargo" || s == "vendor" {
                return true;
            }
        }
    }
    false
}

fn head_lines(path: &str, n: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => content
            .lines()
            .take(n)
            .collect::<Vec<&str>>()
            .join("\n"),
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn tail_lines(path: &str, n: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            let start = lines.len().saturating_sub(n);
            lines[start..].join("\n")
        }
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn slice_file_lines(path: &str, start: usize, end: usize) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().collect();
            if lines.is_empty() {
                return String::new();
            }
            let s = (start.max(1) - 1).min(lines.len().saturating_sub(1));
            let e = end.min(lines.len());
            if s < e {
                lines[s..e].join("\n")
            } else {
                String::new()
            }
        }
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn outline_file(path: &str) -> String {
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let mut matches = Vec::new();
            for (idx, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.starts_with("pub fn ")
                    || trimmed.starts_with("fn ")
                    || trimmed.starts_with("pub struct ")
                    || trimmed.starts_with("struct ")
                    || trimmed.starts_with("pub enum ")
                    || trimmed.starts_with("enum ")
                    || trimmed.starts_with("pub trait ")
                    || trimmed.starts_with("trait ")
                    || trimmed.starts_with("impl ")
                    || trimmed.starts_with("pub mod ")
                    || trimmed.starts_with("class ")
                    || trimmed.starts_with("def ")
                    || trimmed.starts_with('#')
                {
                    matches.push(format!("{}: {}", idx + 1, trimmed));
                }
            }
            matches.join("\n")
        }
        Err(e) => format!("Error reading {path}: {e}"),
    }
}

fn grep_path(target: &str, pattern: &str) -> Dynamic {
    let mut matches = Array::new();
    let pat_lower = pattern.to_lowercase();

    let glob_pat = if target == "." {
        "**/*".to_string()
    } else if target.contains('*') {
        target.to_string()
    } else if Path::new(target).is_file() {
        if let Ok(content) = std::fs::read_to_string(target) {
            for (idx, line) in content.lines().enumerate() {
                if line.to_lowercase().contains(&pat_lower) {
                    let mut m = Map::new();
                    m.insert("path".into(), Dynamic::from(target.to_string()));
                    m.insert("line".into(), Dynamic::from((idx + 1) as i64));
                    m.insert("content".into(), Dynamic::from(line.trim().to_string()));
                    matches.push(Dynamic::from(m));
                }
            }
        }
        return Dynamic::from(matches);
    } else {
        format!("{target}/**/*")
    };

    if let Ok(paths) = glob(&glob_pat) {
        for entry in paths.flatten() {
            if entry.is_file() && !is_ignored_path(&entry) {
                if let Ok(content) = std::fs::read_to_string(&entry) {
                    let path_str = entry.display().to_string();
                    for (idx, line) in content.lines().enumerate() {
                        if line.to_lowercase().contains(&pat_lower) {
                            let mut m = Map::new();
                            m.insert("path".into(), Dynamic::from(path_str.clone()));
                            m.insert("line".into(), Dynamic::from((idx + 1) as i64));
                            m.insert("content".into(), Dynamic::from(line.trim().to_string()));
                            matches.push(Dynamic::from(m));
                            if matches.len() >= 200 {
                                return Dynamic::from(matches);
                            }
                        }
                    }
                }
            }
        }
    }

    Dynamic::from(matches)
}

fn grep_array(files: &[Dynamic], pattern: &str) -> Array {
    let mut matches = Array::new();
    let pat_lower = pattern.to_lowercase();

    for item in files {
        if let Some(m) = item.clone().try_cast::<Map>() {
            if let (Some(path_dyn), Some(content_dyn)) = (m.get("path"), m.get("content")) {
                let path_str = path_dyn.to_string();
                let content_str = content_dyn.to_string();
                for (idx, line) in content_str.lines().enumerate() {
                    if line.to_lowercase().contains(&pat_lower) {
                        let mut hit = Map::new();
                        hit.insert("path".into(), Dynamic::from(path_str.clone()));
                        hit.insert("line".into(), Dynamic::from((idx + 1) as i64));
                        hit.insert("content".into(), Dynamic::from(line.trim().to_string()));
                        matches.push(Dynamic::from(hit));
                    }
                }
            }
        } else if let Ok(s) = item.clone().into_string() {
            if s.to_lowercase().contains(&pat_lower) {
                matches.push(Dynamic::from(s));
            }
        }
    }

    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replace_lines_and_insert() {
        let temp_file = "/tmp/umoja_test_replace_lines.txt";
        let original = "line1\nline2\nline3\nline4\nline5\n";
        std::fs::write(temp_file, original).unwrap();

        // Replace lines 2-4 with new text
        let ok = replace_lines_in_file(temp_file, 2, 4, "replaced_block");
        assert!(ok);

        let content = std::fs::read_to_string(temp_file).unwrap();
        assert_eq!(content, "line1\nreplaced_block\nline5\n");

        // Insert at line 2
        insert_line_at(temp_file, 2, "inserted_line");
        let content2 = std::fs::read_to_string(temp_file).unwrap();
        assert_eq!(content2, "line1\ninserted_line\nreplaced_block\nline5\n");

        // Insert after anchor
        insert_relative_to_anchor(temp_file, "inserted_line", "after_anchor", true);
        let content3 = std::fs::read_to_string(temp_file).unwrap();
        assert_eq!(content3, "line1\ninserted_line\nafter_anchor\nreplaced_block\nline5\n");

        let _ = std::fs::remove_file(temp_file);
    }

    #[test]
    fn test_base64_operations() {
        let temp_file = "/tmp/umoja_test_b64.txt";
        let payload = "Hello \"World\" with \\ special 'quotes'";
        let b64 = BASE64_STANDARD.encode(payload);

        let mut engine = Engine::new();
        register_files_builtins(&mut engine);

        let write_ok = engine.eval::<bool>(&format!("write_b64(\"{temp_file}\", \"{b64}\")")).unwrap();
        assert!(write_ok);

        let read_back = std::fs::read_to_string(temp_file).unwrap();
        assert_eq!(read_back, payload);

        let _ = std::fs::remove_file(temp_file);
    }
}
