//! Every door out of the domain.
//!
//! If a use case needs the world, it needs one of these traits. That is the
//! whole discipline: `pa-app` depends on this file, `pa-infra` implements it,
//! and neither one depends on the other.

use chrono::{DateTime, Utc};

use crate::autonomous::{AutonomousState, Gate, GateOutcome};
use crate::compaction::{CompactionPlan, CompactionState};
use crate::error::Result;
use crate::goal::Goal;
use crate::harness::{HarnessEntry, HarnessScope, Refinement};
use crate::heartbeat::Heartbeat;
use crate::kernel::{ExecOutcome, ExecRequest, KernelLanguage, KernelStatus, VarSummary};
use crate::message::AgentMessage;
use crate::runner::{RunOutcome, RunRequest, RunnerCapabilities};
use crate::schedule::ScheduledJob;
use crate::session::Session;
use crate::skill::SkillManifest;
use crate::subagent::Subagent;
use crate::transcript::TranscriptRecord;

/// Sessions and their identity.
pub trait SessionStore: Send + Sync {
    fn insert(&self, session: &Session) -> Result<()>;
    fn update(&self, session: &Session) -> Result<()>;
    fn get(&self, id: &str) -> Result<Session>;
    /// Resolve an id *or* a name. Returns `NotFound` when neither matches, and
    /// `Conflict` when a name is ambiguous — guessing which agent the user
    /// meant is the one thing worse than asking.
    fn resolve(&self, selector: &str) -> Result<Session>;
    fn list(&self) -> Result<Vec<Session>>;
    fn children_of(&self, parent_id: &str) -> Result<Vec<Session>>;
    fn remove(&self, id: &str) -> Result<()>;
}

/// The append-only record.
pub trait TranscriptLog: Send + Sync {
    fn append(&self, record: &TranscriptRecord) -> Result<()>;
    fn read(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<TranscriptRecord>>;
}

/// Durable supplemental state, per scope.
pub trait HarnessStore: Send + Sync {
    fn upsert(&self, session_id: Option<&str>, entry: &HarnessEntry) -> Result<()>;
    fn get(&self, session_id: Option<&str>, scope: HarnessScope, name: &str) -> Result<HarnessEntry>;
    fn remove(&self, session_id: Option<&str>, scope: HarnessScope, name: &str) -> Result<()>;
    /// Local entries for the session plus every global entry, which is what a
    /// prompt actually wants.
    fn list(&self, session_id: Option<&str>) -> Result<Vec<HarnessEntry>>;

    fn record_refinement(&self, session_id: Option<&str>, refinement: &Refinement) -> Result<()>;
    fn update_refinement(&self, session_id: Option<&str>, refinement: &Refinement) -> Result<()>;
    fn refinements(&self, session_id: Option<&str>, limit: Option<usize>)
        -> Result<Vec<Refinement>>;
    fn refinement(&self, session_id: Option<&str>, id: &str) -> Result<Refinement>;
}

pub trait GoalStore: Send + Sync {
    fn put(&self, goal: &Goal) -> Result<()>;
    fn get(&self, session_id: &str) -> Result<Option<Goal>>;
    fn clear(&self, session_id: &str) -> Result<()>;
    /// Every session with a live goal, for the supervisor tick.
    fn active(&self) -> Result<Vec<Goal>>;
}

pub trait HeartbeatStore: Send + Sync {
    fn put(&self, heartbeat: &Heartbeat) -> Result<()>;
    fn get(&self, id: &str) -> Result<Heartbeat>;
    fn remove(&self, id: &str) -> Result<()>;
    fn list(&self, session_id: Option<&str>) -> Result<Vec<Heartbeat>>;
    fn due(&self, now: DateTime<Utc>) -> Result<Vec<Heartbeat>>;
}

pub trait ScheduleStore: Send + Sync {
    fn put(&self, job: &ScheduledJob) -> Result<()>;
    fn get(&self, id: &str) -> Result<ScheduledJob>;
    fn list(&self, target: Option<&str>, include_finished: bool) -> Result<Vec<ScheduledJob>>;
    fn due(&self, now: DateTime<Utc>) -> Result<Vec<ScheduledJob>>;
    fn remove(&self, id: &str) -> Result<()>;
}

pub trait MessageStore: Send + Sync {
    fn enqueue(&self, message: &AgentMessage) -> Result<()>;
    fn update(&self, message: &AgentMessage) -> Result<()>;
    fn pending_for(&self, session_id: &str) -> Result<Vec<AgentMessage>>;
    fn outbox(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<AgentMessage>>;
    fn get(&self, id: &str) -> Result<AgentMessage>;
}

pub trait SubagentRegistry: Send + Sync {
    fn insert(&self, child: &Subagent) -> Result<()>;
    fn update(&self, child: &Subagent) -> Result<()>;
    fn get(&self, parent_session_id: &str, selector: &str) -> Result<Subagent>;
    fn list(&self, parent_session_id: &str, include_deleted: bool) -> Result<Vec<Subagent>>;
    /// Every child anywhere, for the supervisor and for `agents`.
    fn all(&self) -> Result<Vec<Subagent>>;
}

pub trait AutonomousStore: Send + Sync {
    fn put(&self, state: &AutonomousState) -> Result<()>;
    fn get(&self, session_id: &str) -> Result<Option<AutonomousState>>;
    fn clear(&self, session_id: &str) -> Result<()>;
}

pub trait CompactionStore: Send + Sync {
    fn put(&self, state: &CompactionState) -> Result<()>;
    fn get(&self, session_id: &str) -> Result<Option<CompactionState>>;
}

/// The persistent namespace.
pub trait KernelPort: Send + Sync {
    fn language(&self) -> KernelLanguage;
    fn status(&self, session_id: &str) -> Result<KernelStatus>;
    /// Starts the process if it is not running. Kernels are lazy by design:
    /// a session that never executes code never pays for an interpreter.
    fn ensure(&self, session_id: &str) -> Result<KernelStatus>;
    fn execute(&self, request: &ExecRequest) -> Result<ExecOutcome>;
    /// Names and shapes, never values.
    fn vars(&self, session_id: &str) -> Result<Vec<VarSummary>>;
    /// Empties the namespace, keeps the process.
    fn reset(&self, session_id: &str) -> Result<()>;
    fn shutdown(&self, session_id: &str) -> Result<()>;
    /// Writes the namespace to the session artifact directory so a restarted
    /// worker can revive it. `Unsupported` is a legitimate answer.
    fn snapshot(&self, session_id: &str) -> Result<Option<String>>;
    fn restore(&self, session_id: &str) -> Result<bool>;
}

/// Resolves the harness a particular session belongs to.
///
/// A session records which harness started it, and a later turn — a heartbeat,
/// a goal continuation, a scheduled prompt — must go back to that same one. A
/// supervisor holding a single runner would continue every session under
/// whichever harness the tick happened to be invoked with, which is how a
/// resumed conversation loses its thread.
pub trait RunnerRegistry: Send + Sync {
    fn get(&self, name: &str) -> Result<std::sync::Arc<dyn AgentRunner>>;
    /// The harness used when a session does not name one.
    fn default_name(&self) -> String;
}

/// A harness that can run an agent turn.
pub trait AgentRunner: Send + Sync {
    fn capabilities(&self) -> RunnerCapabilities;
    fn run(&self, request: &RunRequest) -> Result<RunOutcome>;
    /// Is the underlying CLI actually installed and usable?
    fn probe(&self) -> Result<()>;
}

/// Runs autonomous-mode gates and fingerprints the workspace they ran against.
pub trait GateRunner: Send + Sync {
    fn run(&self, gate: &Gate, workdir: &str) -> Result<GateOutcome>;
    fn fingerprint(&self, workdir: &str) -> Result<Option<String>>;
}

/// Finds and parses skills across every location.
pub trait SkillCatalog: Send + Sync {
    fn discover(&self, workdir: &str) -> Result<Vec<SkillManifest>>;
    fn load_body(&self, manifest: &SkillManifest) -> Result<String>;
}

/// Summarises a transcript. Compaction needs a model; the domain only needs to
/// know that something can do it.
pub trait Summariser: Send + Sync {
    fn summarise(&self, plan: &CompactionPlan, records: &[TranscriptRecord]) -> Result<String>;
}

/// Process supervision, for the daemon commands.
pub trait ProcessSupervisor: Send + Sync {
    fn is_alive(&self, pid: u32) -> bool;
    fn terminate(&self, pid: u32, force: bool) -> Result<()>;
    fn current_pid(&self) -> u32;
}
