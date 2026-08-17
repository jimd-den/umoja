//! Where everything lives on disk.
//!
//! The layout mirrors prime-agent's, so a reader who knows one knows the other:
//!
//! ```text
//! ~/.prime/agent/
//!   sessions/<session-id>.jsonl        the transcript, append-only
//!   session-artifacts/<session-id>/
//!     kernel.sock                      the live namespace
//!     kernel-state.json                snapshot for revival
//!     harness/harness_state.json       session-local harness
//!     refinements.jsonl                the paper trail
//!   harness/harness_state.json         global harness
//!   registry/                          sessions, jobs, heartbeats, messages
//!   runtime/                           kernel bootstrap scripts
//! ```

use std::path::{Path, PathBuf};

use pa_domain::error::{DomainError, Result};

#[derive(Debug, Clone)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// `$PRIME_AGENT_HOME`, else `~/.prime/agent`, else `$XDG_DATA_HOME` when
    /// the home directory is not writable.
    pub fn resolve() -> Result<Self> {
        if let Some(explicit) = std::env::var_os("PRIME_AGENT_HOME") {
            return Self::at(PathBuf::from(explicit));
        }

        if let Some(home) = home_dir() {
            let candidate = home.join(".prime").join("agent");
            if Self::at(candidate.clone()).is_ok() {
                return Self::at(candidate);
            }
        }

        let xdg = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home_dir().map(|home| home.join(".local").join("share")))
            .ok_or_else(|| {
                DomainError::adapter("paths", "no home directory and no XDG_DATA_HOME")
            })?;
        Self::at(xdg.join("prime-agent"))
    }

    pub fn at(root: PathBuf) -> Result<Self> {
        let paths = Self { root };
        std::fs::create_dir_all(paths.root())
            .map_err(|error| DomainError::adapter("create prime-agent home", error))?;
        Ok(paths)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn sessions_dir(&self) -> PathBuf {
        self.root.join("sessions")
    }

    pub fn transcript(&self, session_id: &str) -> PathBuf {
        self.sessions_dir().join(format!("{session_id}.jsonl"))
    }

    pub fn artifacts(&self, session_id: &str) -> PathBuf {
        self.root.join("session-artifacts").join(session_id)
    }

    pub fn kernel_socket(&self, session_id: &str) -> PathBuf {
        self.artifacts(session_id).join("kernel.sock")
    }

    pub fn kernel_state(&self, session_id: &str) -> PathBuf {
        self.artifacts(session_id).join("kernel-state.json")
    }

    pub fn kernel_log(&self, session_id: &str) -> PathBuf {
        self.artifacts(session_id).join("kernel.log")
    }

    pub fn local_harness(&self, session_id: &str) -> PathBuf {
        self.artifacts(session_id)
            .join("harness")
            .join("harness_state.json")
    }

    pub fn global_harness(&self) -> PathBuf {
        self.root.join("harness").join("harness_state.json")
    }

    pub fn local_refinements(&self, session_id: &str) -> PathBuf {
        self.artifacts(session_id).join("refinements.jsonl")
    }

    pub fn global_refinements(&self) -> PathBuf {
        self.root.join("harness").join("refinements.jsonl")
    }

    pub fn registry(&self, name: &str) -> PathBuf {
        self.root.join("registry").join(format!("{name}.json"))
    }

    pub fn runtime(&self, name: &str) -> PathBuf {
        self.root.join("runtime").join(name)
    }

    pub fn daemon_dir(&self) -> PathBuf {
        self.root.join("daemon")
    }
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// Creates a file's parent directory, since every write here is to a nested
/// path that may not exist yet.
pub fn ensure_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| DomainError::adapter(format!("create {}", parent.display()), error))?;
    }
    Ok(())
}
