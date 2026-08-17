//! The persistent execution namespace — prompt-as-a-variable.
//!
//! The whole point of this subsystem is a token argument: reading a 200MB log
//! into the conversation costs tokens proportional to its size; loading it into
//! a variable and printing `len(errors)` costs eleven. The kernel is what makes
//! the second option available across separate tool calls.
//!
//! The domain describes the *contract* only. Whether that namespace is held by
//! a Python interpreter, a Node process or a shell is an adapter's problem.

use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KernelLanguage {
    Python,
    Node,
    Shell,
}

impl KernelLanguage {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "python" | "python3" | "py" => Ok(Self::Python),
            "node" | "js" | "javascript" => Ok(Self::Node),
            "shell" | "sh" | "bash" => Ok(Self::Shell),
            other => Err(DomainError::invalid(format!(
                "unknown kernel language '{other}'; expected python, node or shell"
            ))),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Python => "python",
            Self::Node => "node",
            Self::Shell => "shell",
        }
    }

    /// Shell "namespaces" are exported variables and a working directory, not a
    /// heap of live objects. Callers use this to warn rather than to pretend.
    pub fn holds_objects(self) -> bool {
        !matches!(self, Self::Shell)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KernelStatus {
    /// No process yet. Kernels start lazily, on first use.
    Cold,
    Ready,
    Busy,
    /// The process died; the next execution will rebuild it.
    Dead,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stream {
    Stdout,
    Stderr,
}

/// One unit of code to run in the namespace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecRequest {
    pub session_id: String,
    pub code: String,
    pub timeout_secs: u64,
    /// Output beyond this is clipped, with a note saying how much was dropped.
    /// A kernel exists to keep large data *out* of context; a runaway `print`
    /// must not defeat that.
    pub max_output_bytes: usize,
}

impl ExecRequest {
    pub const DEFAULT_TIMEOUT_SECS: u64 = 120;
    pub const DEFAULT_MAX_OUTPUT: usize = 16 * 1024;

    pub fn new(session_id: impl Into<String>, code: impl Into<String>) -> Result<Self> {
        let code = code.into();
        if code.trim().is_empty() {
            return Err(DomainError::invalid("nothing to execute"));
        }
        Ok(Self {
            session_id: session_id.into(),
            code,
            timeout_secs: Self::DEFAULT_TIMEOUT_SECS,
            max_output_bytes: Self::DEFAULT_MAX_OUTPUT,
        })
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs.max(1);
        self
    }

    pub fn with_max_output(mut self, bytes: usize) -> Self {
        self.max_output_bytes = bytes.max(256);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecOutcome {
    pub ok: bool,
    pub stdout: String,
    pub stderr: String,
    /// The repr of the final expression, when there was one.
    pub result: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
    /// Bytes dropped by the output clip, if any.
    #[serde(default)]
    pub truncated_bytes: usize,
    pub timed_out: bool,
}

impl ExecOutcome {
    pub fn failure(error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            ok: false,
            stdout: String::new(),
            stderr: String::new(),
            result: None,
            error: Some(error.into()),
            duration_ms,
            truncated_bytes: 0,
            timed_out: false,
        }
    }

    /// Clips both streams, appending a note that says what was lost. Silence
    /// would be worse than the truncation.
    pub fn clip(mut self, max_bytes: usize) -> Self {
        let (stdout, dropped_out) = clip(&self.stdout, max_bytes);
        let (stderr, dropped_err) = clip(&self.stderr, max_bytes);
        self.stdout = stdout;
        self.stderr = stderr;
        self.truncated_bytes = dropped_out + dropped_err;
        self
    }
}

fn clip(text: &str, max_bytes: usize) -> (String, usize) {
    if text.len() <= max_bytes {
        return (text.to_string(), 0);
    }
    // Keep the head and the tail: a traceback's first and last lines carry most
    // of the meaning, and the middle of a dump rarely does.
    let head_budget = max_bytes * 2 / 3;
    let tail_budget = max_bytes - head_budget;
    let head_end = floor_boundary(text, head_budget);
    let tail_start = ceil_boundary(text, text.len() - tail_budget);
    let dropped = tail_start - head_end;
    let mut out = String::with_capacity(max_bytes + 64);
    out.push_str(&text[..head_end]);
    out.push_str(&format!(
        "\n... [{dropped} bytes clipped; run again printing a smaller slice] ...\n"
    ));
    out.push_str(&text[tail_start..]);
    (out, dropped)
}

fn floor_boundary(text: &str, mut index: usize) -> usize {
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn ceil_boundary(text: &str, mut index: usize) -> usize {
    while index < text.len() && !text.is_char_boundary(index) {
        index += 1;
    }
    index
}

/// What a variable is, without its value.
///
/// `kernel vars` prints these rather than contents on purpose: seeing that
/// `rows` is a 4.2 million element list is the information that decides what to
/// do next, and printing the list would defeat the entire feature.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VarSummary {
    pub name: String,
    pub type_name: String,
    /// `len()` where the value has one.
    pub length: Option<u64>,
    /// Approximate footprint in bytes.
    pub size_bytes: Option<u64>,
    /// A short, deliberately lossy preview.
    pub preview: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_code_is_refused() {
        assert!(ExecRequest::new("ses-1", "   \n").is_err());
    }

    #[test]
    fn clipping_keeps_both_ends_and_says_what_it_dropped() {
        let outcome = ExecOutcome {
            ok: true,
            stdout: "A".repeat(1000) + &"Z".repeat(1000),
            stderr: String::new(),
            result: None,
            error: None,
            duration_ms: 1,
            truncated_bytes: 0,
            timed_out: false,
        }
        .clip(300);

        assert!(outcome.stdout.starts_with("AAA"));
        assert!(outcome.stdout.ends_with("ZZZ"));
        assert!(outcome.stdout.contains("clipped"));
        assert!(outcome.truncated_bytes > 0);
    }

    #[test]
    fn short_output_is_left_alone() {
        let outcome = ExecOutcome {
            ok: true,
            stdout: "hello".into(),
            stderr: String::new(),
            result: None,
            error: None,
            duration_ms: 1,
            truncated_bytes: 0,
            timed_out: false,
        }
        .clip(1024);
        assert_eq!(outcome.stdout, "hello");
        assert_eq!(outcome.truncated_bytes, 0);
    }

    #[test]
    fn clipping_never_splits_a_character() {
        let text = "é".repeat(500);
        let (clipped, _) = clip(&text, 301);
        assert!(clipped.contains('é'));
    }

    #[test]
    fn shell_kernels_admit_they_hold_no_objects() {
        assert!(!KernelLanguage::Shell.holds_objects());
        assert!(KernelLanguage::Python.holds_objects());
    }
}
