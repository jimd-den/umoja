//! JSON and JSONL persistence.
//!
//! Two properties matter and both are about crashes rather than speed:
//!
//! * **Atomic replace.** Every whole-file write goes to a temporary file and is
//!   renamed over the target. A process killed mid-write leaves the previous
//!   version intact rather than a half-serialised registry.
//! * **Append-only where it counts.** Transcripts and refinement logs are only
//!   ever appended to, so evidence cannot be rewritten by a later bug.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use pa_domain::error::{DomainError, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::paths::ensure_parent;

/// Reads a JSON document, treating "not there" as "empty".
///
/// A missing registry is the normal state on a first run, not an error worth
/// making every caller handle.
pub fn read_json<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => Ok(T::default()),
        Ok(text) => serde_json::from_str(&text)
            .map_err(|error| DomainError::parse(path.display().to_string(), error)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(error) => Err(DomainError::adapter(
            format!("read {}", path.display()),
            error,
        )),
    }
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    ensure_parent(path)?;
    let text = serde_json::to_string_pretty(value)
        .map_err(|error| DomainError::adapter("serialise", error))?;

    let temp = path.with_extension(format!(
        "tmp-{}",
        std::process::id()
    ));
    std::fs::write(&temp, text.as_bytes())
        .map_err(|error| DomainError::adapter(format!("write {}", temp.display()), error))?;
    std::fs::rename(&temp, path).map_err(|error| {
        let _ = std::fs::remove_file(&temp);
        DomainError::adapter(format!("replace {}", path.display()), error)
    })
}

/// Appends one JSON line. Never rewrites what is already there.
pub fn append_jsonl<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    ensure_parent(path)?;
    let mut line = serde_json::to_string(value)
        .map_err(|error| DomainError::adapter("serialise", error))?;
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| DomainError::adapter(format!("open {}", path.display()), error))?;
    file.write_all(line.as_bytes())
        .map_err(|error| DomainError::adapter(format!("append {}", path.display()), error))
}

/// Reads a JSONL file, skipping lines that no longer parse.
///
/// A corrupt line in the middle of a transcript must not make the rest of the
/// history unreadable — losing one record beats losing the log.
pub fn read_jsonl<T: DeserializeOwned>(path: &Path, limit: Option<usize>) -> Result<Vec<T>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(DomainError::adapter(
                format!("open {}", path.display()),
                error,
            ))
        }
    };

    let mut rows = Vec::new();
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|error| DomainError::adapter("read line", error))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<T>(&line) {
            rows.push(value);
        }
    }

    if let Some(limit) = limit {
        let start = rows.len().saturating_sub(limit);
        rows.drain(..start);
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_missing_json_file_reads_as_empty() {
        let dir = tempdir("missing");
        let rows: Vec<String> = read_json(&dir.join("nope.json")).unwrap();
        assert!(rows.is_empty());
    }

    #[test]
    fn json_round_trips_through_an_atomic_replace() {
        let dir = tempdir("roundtrip");
        let path = dir.join("nested/deep/registry.json");
        write_json(&path, &vec!["a".to_string(), "b".to_string()]).unwrap();
        let rows: Vec<String> = read_json(&path).unwrap();
        assert_eq!(rows, vec!["a", "b"]);
        // No temporary files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("tmp-"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn jsonl_appends_and_reads_back_in_order() {
        let dir = tempdir("jsonl");
        let path = dir.join("log.jsonl");
        for value in ["first", "second", "third"] {
            append_jsonl(&path, &value.to_string()).unwrap();
        }
        let rows: Vec<String> = read_jsonl(&path, None).unwrap();
        assert_eq!(rows, vec!["first", "second", "third"]);

        let tail: Vec<String> = read_jsonl(&path, Some(2)).unwrap();
        assert_eq!(tail, vec!["second", "third"]);
    }

    #[test]
    fn a_corrupt_line_does_not_hide_the_rest_of_the_log() {
        let dir = tempdir("corrupt");
        let path = dir.join("log.jsonl");
        append_jsonl(&path, &"good".to_string()).unwrap();
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{not json at all\n")
            .unwrap();
        append_jsonl(&path, &"also good".to_string()).unwrap();

        let rows: Vec<String> = read_jsonl(&path, None).unwrap();
        assert_eq!(rows, vec!["good", "also good"]);
    }

    #[test]
    fn malformed_json_is_a_parse_error_not_a_default() {
        let dir = tempdir("malformed");
        let path = dir.join("registry.json");
        std::fs::write(&path, b"{oh dear").unwrap();
        let result: Result<Vec<String>> = read_json(&path);
        assert!(matches!(result, Err(DomainError::Parse { .. })));
    }
}
