//! The command surface.
//!
//! Every capability prime-agent exposes has a command here, named the same way
//! wherever the name still makes sense outside a TUI. Where prime-agent has a
//! slash command (`/goal`, `/refine`, `/autonomous`), this has a subcommand,
//! because a one-shot binary invoked by an agent has no session to slash into.

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "pa",
    version,
    about = "Prime Agent capabilities as a portable, harness-agnostic tool",
    long_about = "A persistent kernel, a continual harness, subagents, goals, \
                  heartbeats, schedules, messaging, autonomous gates and \
                  compaction — usable from Claude Code, opencode or a plain shell."
)]
pub struct Cli {
    /// Where state lives. Defaults to $PRIME_AGENT_HOME, then ~/.prime/agent.
    #[arg(long, global = true, value_name = "DIR")]
    pub home: Option<String>,

    /// Which session to act on: its name or its id. Defaults to $PA_SESSION,
    /// then to the session named after the working directory.
    #[arg(long, short = 's', global = true, value_name = "SELECTOR")]
    pub session: Option<String>,

    /// Which harness runs agent turns: claude, opencode or dry-run.
    #[arg(long, global = true, value_name = "NAME")]
    pub runner: Option<String>,

    /// Working directory for runs, gates and skill discovery.
    #[arg(long, short = 'C', global = true, value_name = "DIR")]
    pub workdir: Option<String>,

    /// Emit JSON instead of text. Every command supports it.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Start a session.
    Start(StartArgs),
    /// List sessions.
    #[command(alias = "list")]
    Agents(AgentsArgs),
    /// Give a session a stable readable name.
    Rename { selector: String, name: String },
    /// Stop a session and its worker.
    Stop {
        selector: String,
        #[arg(long)]
        force: bool,
    },
    /// Show what is running, scheduled and outstanding.
    Status,
    /// Find and optionally repair inconsistent state.
    Doctor {
        #[arg(long)]
        fix: bool,
    },
    /// Stop every session and kernel.
    Shutdown {
        #[arg(long)]
        force: bool,
    },
    /// Print a session's transcript.
    Log {
        #[arg(long, short = 'n', default_value_t = 40)]
        lines: usize,
        /// Keep printing new events as they land, until interrupted.
        #[arg(long, short = 'f')]
        follow: bool,
    },

    /// Reattach to a session: what it is, what it has, and what it does next.
    ///
    /// The terminal that started a session does not own it — the work runs
    /// detached and the state is on disk — so this can be run from anywhere,
    /// any time, including after the original shell is gone.
    Attach {
        /// A session name or id. Defaults to this directory's session.
        selector: Option<String>,
        /// Print the backlog and exit instead of following.
        #[arg(long)]
        no_follow: bool,
        /// How much backlog to show before following.
        #[arg(long, short = 'n', default_value_t = 20)]
        lines: usize,
    },

    /// The persistent namespace: prompt-as-a-variable.
    #[command(subcommand)]
    Kernel(KernelCommand),
    /// Durable supplemental state: notes, memories, skills, subagent specs.
    #[command(subcommand)]
    Harness(HarnessCommand),
    /// The record of harness changes, and how to undo one.
    #[command(subcommand)]
    Refine(RefineCommand),
    /// Recursive delegation.
    #[command(subcommand)]
    Agent(AgentCommand),
    /// Persistent objectives.
    #[command(subcommand)]
    Goal(GoalCommand),
    /// Recurring instructions.
    #[command(subcommand)]
    Heartbeat(HeartbeatCommand),
    /// One-time and cron prompts.
    #[command(subcommand)]
    Schedule(ScheduleCommand),
    /// Bounded autonomous continuation.
    #[command(subcommand)]
    Autonomous(AutonomousCommand),
    /// Context compaction.
    #[command(subcommand)]
    Compact(CompactCommand),
    /// Installed skills, across every harness's directories.
    #[command(subcommand)]
    Skills(SkillsCommand),

    /// Send a message to another agent.
    Send(SendArgs),
    /// Read messages addressed to this session.
    Inbox {
        /// Mark them read and remove them from the queue.
        #[arg(long)]
        consume: bool,
    },
    /// Everyone this session may address.
    Roster,

    /// Deliver everything that is due: heartbeats, schedules, goals.
    Tick {
        /// Report what would be delivered without running anything.
        #[arg(long)]
        dry_run: bool,
    },

    /// Print the supplemental prompt block: harness, skills and any live goal.
    Prompt,
}

#[derive(Debug, Args)]
pub struct StartArgs {
    /// A readable name. Derived from the working directory when omitted.
    pub name: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
}

#[derive(Debug, Args)]
pub struct AgentsArgs {
    /// Include sessions that have finished.
    #[arg(long, short = 'a')]
    pub all: bool,
}

#[derive(Debug, Subcommand)]
pub enum KernelCommand {
    /// Run code in the session's namespace. `-` reads from stdin.
    Exec {
        code: String,
        #[arg(long, default_value_t = 120)]
        timeout: u64,
        /// python, node or shell.
        #[arg(long, short = 'l')]
        lang: Option<String>,
        /// Clip output at this many bytes.
        #[arg(long, default_value_t = 16384)]
        max_output: usize,
    },
    /// List what is bound, with shapes and sizes but never values.
    Vars {
        #[arg(long, short = 'l')]
        lang: Option<String>,
    },
    /// Empty the namespace, keep the process.
    Reset {
        #[arg(long, short = 'l')]
        lang: Option<String>,
    },
    /// Is the kernel cold, ready or dead?
    Status {
        #[arg(long, short = 'l')]
        lang: Option<String>,
    },
    /// Stop the kernel process.
    Stop {
        #[arg(long, short = 'l')]
        lang: Option<String>,
    },
    /// Write the namespace to disk so it can be revived.
    Snapshot {
        #[arg(long, short = 'l')]
        lang: Option<String>,
    },
    /// Load a snapshot back into a kernel.
    Restore {
        #[arg(long, short = 'l')]
        lang: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum HarnessCommand {
    /// List entries visible to this session.
    List {
        /// prompt-note, memory, skill or subagent.
        #[arg(long)]
        kind: Option<String>,
    },
    /// Show one entry in full.
    Show { name: String },
    /// Write something down. Requires evidence.
    Remember(RememberArgs),
    /// Remove an entry, reversibly.
    Forget {
        name: String,
        #[arg(long, default_value = "local")]
        scope: String,
        /// Why it is no longer true.
        #[arg(long)]
        evidence: String,
    },
    /// Print the harness as a prompt block.
    Prompt,
}

#[derive(Debug, Args)]
pub struct RememberArgs {
    pub name: String,
    pub body: String,
    /// What in this session justifies writing this down.
    #[arg(long)]
    pub evidence: String,
    /// prompt-note, memory, skill or subagent.
    #[arg(long, default_value = "memory")]
    pub kind: String,
    /// local (this session) or global (the machine).
    #[arg(long, default_value = "local")]
    pub scope: String,
    /// What should improve, and how you would check.
    #[arg(long)]
    pub outcome: Option<String>,
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
}

#[derive(Debug, Subcommand)]
pub enum RefineCommand {
    /// Read this session's trajectory back and propose what is worth
    /// remembering. Proposes only, unless `--apply` is given.
    Review {
        /// How many recent transcript records to review.
        #[arg(long, short = 'n')]
        window: Option<usize>,
        /// Write the proposals as harness entries, one refinement each.
        #[arg(long)]
        apply: bool,
        /// Print the reviewer's reply verbatim, before it was parsed.
        #[arg(long)]
        raw: bool,
    },
    /// Recent harness changes, newest first.
    List {
        #[arg(long, short = 'n', default_value_t = 20)]
        limit: usize,
        /// Include the machine-wide log as well.
        #[arg(long)]
        global: bool,
    },
    /// Show one refinement, with its before and after.
    Show { id: String },
    /// Undo a refinement. The rollback is itself recorded.
    Rollback { id: String },
}

#[derive(Debug, Subcommand)]
pub enum AgentCommand {
    /// Admit a child agent. Returns a handle, never an answer.
    Spawn(SpawnArgs),
    /// Delegate and wait, returning the child's answer. The blocking form of
    /// `spawn`, for when the answer is the input to the next step.
    Call(CallArgs),
    /// This session's children.
    List {
        #[arg(long)]
        all: bool,
    },
    /// Record a child finishing, folding its usage into this session.
    Settle {
        selector: String,
        /// completed, failed or cancelled.
        #[arg(long, default_value = "completed")]
        status: String,
        #[arg(long, default_value_t = 0)]
        input_tokens: u64,
        #[arg(long, default_value_t = 0)]
        output_tokens: u64,
    },
    /// Stop addressing a child. Its transcript is kept.
    Delete { selector: String },
}

#[derive(Debug, Args)]
pub struct SpawnArgs {
    pub prompt: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    /// Override the harness this child runs on.
    #[arg(long)]
    pub with: Option<String>,
    #[arg(long)]
    pub system_prompt: Option<String>,
}

#[derive(Debug, Args)]
pub struct CallArgs {
    pub prompt: String,
    #[arg(long)]
    pub name: Option<String>,
    #[arg(long)]
    pub model: Option<String>,
    /// Override the harness this child runs on.
    #[arg(long)]
    pub with: Option<String>,
    #[arg(long)]
    pub system_prompt: Option<String>,
    /// Give up after this long. The child is still recorded as failed.
    #[arg(long)]
    pub timeout: Option<u64>,
}

#[derive(Debug, Args)]
pub struct SendArgs {
    /// A session name, or `all` to reach this session's family.
    pub target: String,
    pub message: String,
    /// parent, child, sibling, peer or all.
    #[arg(long, default_value = "peer")]
    pub role: String,
    /// auto, steer or follow-up.
    #[arg(long, default_value = "auto")]
    pub mode: String,
}

#[derive(Debug, Subcommand)]
pub enum GoalCommand {
    /// Set the objective.
    Set {
        objective: String,
        /// Token budget.
        #[arg(long)]
        budget: Option<u64>,
        /// Wall-clock budget, e.g. 2h.
        #[arg(long)]
        deadline: Option<String>,
        /// Replace an existing goal.
        #[arg(long)]
        replace: bool,
    },
    Status,
    Pause,
    Resume,
    /// The only way to mark a goal successful.
    Complete,
    Clear,
}

#[derive(Debug, Subcommand)]
pub enum HeartbeatCommand {
    /// Set the user's single visible heartbeat, replacing any previous one.
    Set(HeartbeatArgs),
    /// Add an agent-owned heartbeat alongside the others.
    Add(HeartbeatArgs),
    List {
        #[arg(long)]
        all: bool,
    },
    Pause { id: String },
    Resume { id: String },
    Remove { id: String },
    /// Clear the user's heartbeat.
    Clear,
}

#[derive(Debug, Args)]
pub struct HeartbeatArgs {
    pub prompt: String,
    /// How often: 30s, 10m, 1h30m.
    #[arg(long, default_value = "10m")]
    pub every: String,
    #[arg(long)]
    pub label: Option<String>,
    /// auto, steer or follow-up.
    #[arg(long, default_value = "auto")]
    pub mode: String,
}

#[derive(Debug, Subcommand)]
pub enum ScheduleCommand {
    /// Schedule a prompt: "in 30m", an RFC 3339 instant, or a cron expression.
    Add {
        target: String,
        when: String,
        prompt: String,
        #[arg(long, default_value = "auto")]
        mode: String,
    },
    List {
        #[arg(long)]
        all: bool,
        /// Only jobs for one agent.
        #[arg(long)]
        target: Option<String>,
    },
    Cancel { id: String },
}

#[derive(Debug, Subcommand)]
pub enum AutonomousCommand {
    /// Turn it on for this session.
    On {
        /// A command that must pass before the run may finish. Repeatable.
        #[arg(long = "gate")]
        gates: Vec<String>,
        #[arg(long, default_value_t = 10)]
        max_continuations: u32,
        #[arg(long, default_value_t = 20)]
        max_turns: u32,
        #[arg(long, default_value_t = 500_000)]
        max_tokens: u64,
        /// Wall-clock limit, e.g. 1h.
        #[arg(long, default_value = "1h")]
        max_time: String,
    },
    Off,
    Status,
    /// Run the gates and decide whether to continue.
    Step,
}

#[derive(Debug, Subcommand)]
pub enum CompactCommand {
    Status,
    /// Summarise older context and continue.
    Run {
        /// What the summary must preserve.
        #[arg(long)]
        instruction: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub enum SkillsCommand {
    List,
    /// Print a skill's full instructions.
    Show { name: String },
    /// Print the startup block: one line per skill.
    Prompt,
}
