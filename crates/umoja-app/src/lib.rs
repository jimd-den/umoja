//! Prime Agent use cases.
//!
//! Each service here orchestrates domain entities through domain ports and
//! nothing else. There is no `std::fs`, no `Command`, no `println!` in this
//! crate — which is why every one of these behaviours can be tested against
//! in-memory doubles, and why swapping Claude Code for opencode changes nothing
//! above [`umoja_domain::ports`].

#![forbid(unsafe_code)]

pub mod harness;
pub mod kernel;
pub mod autonomy;
pub mod compaction;
pub mod goals;
pub mod heartbeats;
pub mod messaging;
pub mod schedules;
pub mod review;
pub mod skills;
pub mod sessions;
pub mod subagents;
pub mod supervisor;

use std::sync::Arc;

use umoja_domain::clock::Clock;
use umoja_domain::ids::IdGen;

/// The two dependencies every service needs, bundled so constructors stay
/// readable.
#[derive(Clone)]
pub struct Env {
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn IdGen>,
}

impl Env {
    pub fn new(clock: Arc<dyn Clock>, ids: Arc<dyn IdGen>) -> Self {
        Self { clock, ids }
    }

    pub fn now(&self) -> chrono::DateTime<chrono::Utc> {
        self.clock.now()
    }

    pub fn id(&self, prefix: &str) -> String {
        self.ids.next(prefix)
    }
}

impl std::fmt::Debug for Env {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Env")
    }
}

#[cfg(test)]
pub(crate) mod doubles;
pub mod evolution;
