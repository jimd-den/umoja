use thiserror::Error;

pub type Result<T> = std::result::Result<T, DomainError>;

/// Every way this system is allowed to fail.
///
/// Adapters map their own errors (io, serde, exit codes) into these variants at
/// the boundary, so use cases never match on an `io::Error` and the CLI has a
/// single exhaustive place to decide exit codes and phrasing.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DomainError {
    /// The input was wrong in a way the caller can fix by typing something else.
    #[error("{0}")]
    Invalid(String),

    /// The thing asked for is not there.
    #[error("{kind} '{id}' not found")]
    NotFound { kind: &'static str, id: String },

    /// The thing asked for is already there, and silently overwriting it would
    /// lose something.
    #[error("{kind} '{id}' already exists")]
    Conflict { kind: &'static str, id: String },

    /// The request was well-formed but the current state forbids it — a paused
    /// goal being completed, a subagent recursing past its depth.
    #[error("{0}")]
    Forbidden(String),

    /// A limit deliberately configured by the user was reached. Distinct from
    /// `Forbidden` because hitting a budget is a normal outcome, not a mistake.
    #[error("{limit} limit reached ({reached} of {allowed})")]
    LimitReached {
        limit: &'static str,
        reached: u64,
        allowed: u64,
    },

    /// The outside world failed: disk, process, network, interpreter.
    #[error("{context}: {detail}")]
    Adapter { context: String, detail: String },

    /// Stored state could not be understood.
    #[error("could not parse {what}: {detail}")]
    Parse { what: String, detail: String },

    /// A capability was asked for that this build or environment does not have.
    #[error("{0}")]
    Unsupported(String),
}

impl DomainError {
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    pub fn not_found(kind: &'static str, id: impl Into<String>) -> Self {
        Self::NotFound {
            kind,
            id: id.into(),
        }
    }

    pub fn conflict(kind: &'static str, id: impl Into<String>) -> Self {
        Self::Conflict {
            kind,
            id: id.into(),
        }
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::Forbidden(message.into())
    }

    pub fn adapter(context: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        Self::Adapter {
            context: context.into(),
            detail: detail.to_string(),
        }
    }

    pub fn parse(what: impl Into<String>, detail: impl std::fmt::Display) -> Self {
        Self::Parse {
            what: what.into(),
            detail: detail.to_string(),
        }
    }

    /// True when retrying the identical command could plausibly work. The CLI
    /// uses this to choose between "that failed" and "that will keep failing".
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Adapter { .. })
    }
}
