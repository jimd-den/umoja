//! File inspection, multi-file loading, and search operations for Rhai.

use std::path::Path;
use std::process::Command;

use glob::glob;
use rhai::{Array, Dynamic, Engine, Map};
use serde_json::Value;

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
        std::fs::write(path, content).is_ok()
    });

    engine.register_fn("edit", |path: &str, old_text: &str, new_text: &str| -> bool {
        if let Ok(content) = std::fs::read_to_string(path) {
            if content.contains(old_text) {
                let replaced = content.replacen(old_text, new_text, 1);
                return std::fs::write(path, replaced).is_ok();
            }
        }
        false
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
