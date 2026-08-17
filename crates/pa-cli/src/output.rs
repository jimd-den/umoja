//! Two renderings of every answer.
//!
//! A command produces one [`Output`] carrying both a human line and a JSON
//! value. Neither is a translation of the other after the fact, so `--json`
//! never has to reverse-engineer prose, and the text is free to be terse where
//! the JSON is complete.

use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Output {
    pub text: String,
    pub data: Value,
    /// A non-zero exit code for a command that ran fine but reported a
    /// negative answer — a failing gate, a dead kernel. Scripts need to branch
    /// on that without parsing the text.
    pub code: i32,
}

impl Output {
    pub fn new(text: impl Into<String>, data: Value) -> Self {
        Self {
            text: text.into(),
            data,
            code: 0,
        }
    }

    pub fn message(text: impl Into<String>) -> Self {
        let text = text.into();
        Self {
            data: json!({ "message": text }),
            text,
            code: 0,
        }
    }

    pub fn with_code(mut self, code: i32) -> Self {
        self.code = code;
        self
    }

    pub fn print(&self, as_json: bool) {
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&self.data).unwrap_or_else(|_| "{}".into())
            );
        } else if !self.text.is_empty() {
            println!("{}", self.text);
        }
    }
}

/// Renders rows as an aligned table, which is the whole of the formatting this
/// tool needs.
pub fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    let mut widths: Vec<usize> = headers.iter().map(|header| header.chars().count()).collect();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                widths[index] = widths[index].max(cell.chars().count());
            }
        }
    }

    let mut out = String::new();
    for (index, header) in headers.iter().enumerate() {
        out.push_str(&pad(&header.to_uppercase(), widths[index], index + 1 == headers.len()));
    }
    out.push('\n');

    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if index < widths.len() {
                out.push_str(&pad(cell, widths[index], index + 1 == row.len()));
            }
        }
        out.push('\n');
    }

    out.trim_end().to_string()
}

fn pad(cell: &str, width: usize, last: bool) -> String {
    if last {
        return cell.to_string();
    }
    let gap = width.saturating_sub(cell.chars().count()) + 2;
    format!("{cell}{}", " ".repeat(gap))
}

/// Shortens a value to one readable line.
pub fn clip(text: &str, max: usize) -> String {
    let flat = text.replace(['\n', '\r'], " ");
    let flat = flat.trim();
    if flat.chars().count() <= max {
        return flat.to_string();
    }
    flat.chars().take(max.saturating_sub(1)).chain("…".chars()).collect()
}

/// "3m ago", "2h ago" — relative times read faster than timestamps in a list.
pub fn ago(then: chrono::DateTime<chrono::Utc>, now: chrono::DateTime<chrono::Utc>) -> String {
    let seconds = (now - then).num_seconds();
    if seconds < 0 {
        return format!("in {}", duration(-seconds));
    }
    if seconds < 5 {
        return "just now".into();
    }
    format!("{} ago", duration(seconds))
}

pub fn duration(seconds: i64) -> String {
    match seconds {
        s if s < 60 => format!("{s}s"),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s => format!("{}d", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_table_aligns_its_columns() {
        let rendered = table(
            &["name", "status"],
            &[
                vec!["api-reviewer".into(), "running".into()],
                vec!["x".into(), "idle".into()],
            ],
        );
        let lines: Vec<&str> = rendered.lines().collect();
        assert!(lines[0].starts_with("NAME"));
        assert!(lines[1].starts_with("api-reviewer  running"));
        assert!(lines[2].starts_with("x             idle"));
    }

    #[test]
    fn an_empty_table_renders_nothing() {
        assert!(table(&["a"], &[]).is_empty());
    }

    #[test]
    fn clipping_flattens_and_truncates() {
        assert_eq!(clip("one\ntwo", 40), "one two");
        assert_eq!(clip("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn relative_times_read_as_english() {
        let now = chrono::Utc::now();
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now - chrono::Duration::minutes(3), now), "3m ago");
        assert_eq!(ago(now - chrono::Duration::hours(5), now), "5h ago");
        assert_eq!(ago(now + chrono::Duration::minutes(10), now), "in 10m");
    }
}
