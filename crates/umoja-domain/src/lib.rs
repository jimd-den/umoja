//! The Prime Agent domain.
//!
//! This crate is the centre of the onion: entities, the invariants that make
//! them valid, and the *ports* through which the outside world is reached. It
//! performs no I/O, spawns no processes, reads no clock and knows nothing about
//! Claude Code, opencode, Python, JSON files or the CLI. Everything it needs
//! from the world arrives through a trait in [`ports`].
//!
//! The practical consequence is that every rule worth arguing about — when a
//! goal may continue, whether a subagent may recurse, which skill wins a name
//! collision, when a cron tick is due — is unit-testable without a filesystem.

#![forbid(unsafe_code)]

pub mod autonomous;
pub mod clock;
pub mod compaction;
pub mod error;
pub mod goal;
pub mod harness;
pub mod heartbeat;
pub mod ids;
pub mod kernel;
pub mod message;
pub mod ports;
pub mod report;
pub mod runner;
pub mod schedule;
pub mod session;
pub mod skill;
pub mod subagent;
pub mod timespec;
pub mod transcript;

pub use error::{DomainError, Result};

/// Re-exported so adapters and use cases can `use umoja_domain::prelude::*` and get
/// the whole vocabulary without a wall of imports.
pub mod prelude {
    pub use crate::autonomous::{
        AutonomousLimits, AutonomousPolicy, AutonomousState, Continuation, Gate, GateOutcome,
    };
    pub use crate::clock::Clock;
    pub use crate::compaction::{CompactionPlan, CompactionState, CompactionTrigger};
    pub use crate::error::{DomainError, Result};
    pub use crate::goal::{Goal, GoalBudget, GoalProgress, GoalStatus};
    pub use crate::harness::{
        EntryKind, HarnessEntry, HarnessScope, Proposal, Refinement, RefinementOp, Snapshot,
    };
    pub use crate::heartbeat::{DeliveryMode, Heartbeat, HeartbeatOwner, HeartbeatStatus};
    pub use crate::ids::{IdGen, Ids};
    pub use crate::kernel::{
        ExecOutcome, ExecRequest, KernelLanguage, KernelStatus, Stream, VarSummary,
    };
    pub use crate::message::MessageLimits;
    pub use crate::message::{AgentMessage, DeliveryStatus, Receipt, ReceiverRole};
    pub use crate::ports::*;
    pub use crate::report::{Report, ReportKind, ReportStatus};
    pub use crate::runner::{RunOutcome, RunRequest, RunnerCapabilities};
    pub use crate::schedule::{JobStatus, ScheduleSpec, ScheduledJob};
    pub use crate::session::{Session, SessionKind, SessionStatus, Usage};
    pub use crate::skill::{SkillKind, SkillManifest, SkillSource, Validation};
    pub use crate::subagent::{
        CallResult, DepthPolicy, SpawnHandle, Subagent, SubagentSpec, SubagentStatus,
    };
    pub use crate::timespec::{CronExpr, Interval};
    pub use crate::transcript::{TranscriptEvent, TranscriptRecord};
}
