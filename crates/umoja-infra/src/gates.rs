//! Quality gates and the workspace fingerprint that decides when to re-run one.

use std::process::{Command, Stdio};
use std::time::SystemTime;

use chrono::Utc;
use umoja_domain::autonomous::{Gate, GateOutcome};
use umoja_domain::error::{DomainError, Result};
use umoja_domain::ports::GateRunner;

use crate::hash::{digest, digest_u64};
use crate::kernel::socket::which;

pub struct ShellGateRunner {
    shell: String,
}

impl std::fmt::Debug for ShellGateRunner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ShellGateRunner")
    }
}

impl Default for ShellGateRunner {
    fn default() -> Self {
        Self {
            shell: std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string()),
        }
    }
}

impl GateRunner for ShellGateRunner {
    fn run(&self, gate: &Gate, workdir: &str) -> Result<GateOutcome> {
        let output = Command::new(&self.shell)
            .arg("-c")
            .arg(&gate.command)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .output()
            .map_err(|error| DomainError::adapter(format!("run gate '{}'", gate.command), error))?;

        // stdout and stderr are interleaved into one blob because a failing
        // build usually splits its message across both, and the agent needs the
        // whole message to fix it.
        let mut blob = String::from_utf8_lossy(&output.stdout).to_string();
        let errors = String::from_utf8_lossy(&output.stderr);
        if !errors.trim().is_empty() {
            if !blob.is_empty() {
                blob.push('\n');
            }
            blob.push_str(&errors);
        }

        Ok(GateOutcome {
            command: gate.command.clone(),
            passed: output.status.success(),
            exit_code: output.status.code(),
            output: clip_tail(&blob, gate.max_output_bytes),
            workspace_fingerprint: self.fingerprint(workdir)?,
            ran_at: Utc::now(),
        })
    }

    /// A cheap answer to "has anything changed since the last run?".
    ///
    /// `git status --porcelain` plus HEAD is exact inside a repository and
    /// costs nothing. Outside one it falls back to the newest modification time
    /// in the tree, which is coarser but still catches an edit.
    fn fingerprint(&self, workdir: &str) -> Result<Option<String>> {
        if which("git").is_some() {
            let status = Command::new("git")
                .args(["status", "--porcelain=v1", "--untracked-files=all"])
                .current_dir(workdir)
                .stdin(Stdio::null())
                .output();

            if let Ok(status) = status {
                if status.status.success() {
                    let head = Command::new("git")
                        .args(["rev-parse", "HEAD"])
                        .current_dir(workdir)
                        .stdin(Stdio::null())
                        .output()
                        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                        .unwrap_or_default();
                    let dirty = String::from_utf8_lossy(&status.stdout);
                    return Ok(Some(digest(&format!("{head}\n{dirty}"))));
                }
            }
        }

        Ok(tree_digest(std::path::Path::new(workdir), 0).map(|hash| format!("{hash:016x}")))
    }
}

/// Keeps the tail of a gate's output.
///
/// The last lines of a failing build are the ones naming the error; the first
/// lines are usually the toolchain announcing itself.
fn clip_tail(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "... [{} earlier bytes clipped] ...\n{}",
        start,
        &text[start..]
    )
}

/// A fingerprint of every file in the tree: name, size and nanosecond mtime.
///
/// Nanoseconds rather than seconds because two `pa autonomous step` calls can
/// easily straddle a single edit within the same second, and a fingerprint that
/// cannot see that edit would wrongly report the gate as stale and skip the one
/// run that would have passed. Per-file digests are combined by addition, so
/// the result does not depend on the order `read_dir` happens to return.
fn tree_digest(dir: &std::path::Path, depth: usize) -> Option<u64> {
    if depth > 6 {
        return None;
    }
    let entries = std::fs::read_dir(dir).ok()?;
    let mut combined: u64 = 0;

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Directories that churn on their own would make every fingerprint
        // unique and defeat the whole optimisation.
        if matches!(name.as_ref(), ".git" | "target" | "node_modules" | ".venv") {
            continue;
        }

        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            if let Some(nested) = tree_digest(&entry.path(), depth + 1) {
                combined = combined.wrapping_add(nested);
            }
            continue;
        }

        let nanos = meta
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|elapsed| elapsed.as_nanos())
            .unwrap_or_default();
        combined = combined.wrapping_add(digest_u64(&format!(
            "{name}|{}|{nanos}",
            meta.len()
        )));
    }

    (combined != 0).then_some(combined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workdir(name: &str) -> String {
        let dir = std::env::temp_dir().join(format!("pa-gate-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn a_passing_gate_reports_success_and_a_fingerprint() {
        let dir = workdir("pass");
        std::fs::write(std::path::Path::new(&dir).join("file.txt"), b"hello").unwrap();

        let outcome = ShellGateRunner::default()
            .run(&Gate::new("true").unwrap(), &dir)
            .unwrap();
        assert!(outcome.passed);
        assert_eq!(outcome.exit_code, Some(0));
        assert!(outcome.workspace_fingerprint.is_some());
    }

    #[test]
    fn a_failing_gate_keeps_its_message_from_both_streams() {
        let dir = workdir("fail");
        let outcome = ShellGateRunner::default()
            .run(
                &Gate::new("echo out; echo err >&2; exit 7").unwrap(),
                &dir,
            )
            .unwrap();
        assert!(!outcome.passed);
        assert_eq!(outcome.exit_code, Some(7));
        assert!(outcome.output.contains("out"));
        assert!(outcome.output.contains("err"));
    }

    #[test]
    fn long_output_keeps_its_tail() {
        let clipped = clip_tail(&format!("{}THE ERROR", "noise\n".repeat(5000)), 64);
        assert!(clipped.ends_with("THE ERROR"));
        assert!(clipped.contains("clipped"));
    }

    #[test]
    fn the_fingerprint_notices_an_edit_made_in_the_same_second() {
        // The regression this guards: with second-granular mtimes, editing a
        // file between two quick steps looked like no change at all, and the
        // gate that would now pass was skipped instead of re-run.
        let dir = workdir("fingerprint");
        let runner = ShellGateRunner::default();
        let file = std::path::Path::new(&dir).join("check.sh");

        std::fs::write(&file, b"exit 3").unwrap();
        let before = runner.fingerprint(&dir).unwrap();
        std::fs::write(&file, b"exit 0").unwrap();
        let after = runner.fingerprint(&dir).unwrap();

        assert!(before.is_some());
        assert_ne!(before, after);
    }

    #[test]
    fn an_untouched_workspace_fingerprints_the_same_twice() {
        let dir = workdir("stable-fingerprint");
        std::fs::write(std::path::Path::new(&dir).join("a.txt"), b"one").unwrap();
        let runner = ShellGateRunner::default();
        assert_eq!(
            runner.fingerprint(&dir).unwrap(),
            runner.fingerprint(&dir).unwrap()
        );
    }

}
