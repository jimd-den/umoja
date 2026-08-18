//! The composition root.
//!
//! This is the only place that knows both halves of the system: it picks the
//! adapters and hands them to the use cases. Every `Arc<dyn Port>` in the
//! program is created here and nowhere else, which is what keeps the
//! dependency arrows pointing inwards.

use std::sync::Arc;

use pa_app::autonomy::AutonomyService;
use pa_app::compaction::CompactionService;
use pa_app::goals::GoalService;
use pa_app::harness::HarnessService;
use pa_app::review::ReviewService;
use pa_app::heartbeats::HeartbeatService;
use pa_app::kernel::KernelService;
use pa_app::messaging::MessagingService;
use pa_app::schedules::ScheduleService;
use pa_app::sessions::{SessionService, StartSession};
use pa_app::skills::SkillService;
use pa_app::subagents::SubagentService;
use pa_app::supervisor::SupervisorService;
use pa_app::Env;
use pa_domain::message::MessageLimits;
use pa_domain::prelude::*;
use pa_infra::gates::ShellGateRunner;
use pa_infra::paths::Paths;
use pa_infra::runners::{self, CachingRunnerRegistry};
use pa_infra::skills_fs::FsSkillCatalog;
use pa_infra::stores::*;
use pa_infra::summariser::AgentSummariser;
use pa_infra::sys::{SystemClock, TimeOrderedIds, UnixProcessSupervisor};

use crate::cli::Cli;

/// A context window assumed when a runner does not report one.
const DEFAULT_CONTEXT_WINDOW: u64 = 200_000;

pub struct App {
    pub paths: Paths,
    pub env: Env,
    pub workdir: String,
    pub runner_name: String,

    pub sessions: Arc<dyn SessionStore>,
    pub transcript: Arc<dyn TranscriptLog>,
    pub runner: Arc<dyn AgentRunner>,

    pub session_service: SessionService,
    pub harness: Arc<HarnessService>,
    pub review: ReviewService,
    pub subagents: SubagentService,
    pub messaging: MessagingService,
    pub goals: Arc<GoalService>,
    pub heartbeats: Arc<HeartbeatService>,
    pub schedules: Arc<ScheduleService>,
    pub autonomy: Arc<AutonomyService>,
    pub compaction: CompactionService,
    pub skills: SkillService,
    pub supervisor: SupervisorService,

    session_override: Option<String>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("App")
    }
}

impl App {
    pub fn build(cli: &Cli) -> Result<Self> {
        let paths = match &cli.home {
            Some(home) => Paths::at(home.into())?,
            None => Paths::resolve()?,
        };

        let workdir = cli.workdir.clone().unwrap_or_else(|| {
            std::env::current_dir()
                .map(|dir| dir.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string())
        });

        let env = Env::new(Arc::new(SystemClock), Arc::new(TimeOrderedIds::default()));

        // The runner is chosen once, here: an explicit --runner, then
        // $PA_RUNNER, then whichever harness is actually installed.
        let runner_name = cli
            .runner
            .clone()
            .or_else(|| std::env::var("PA_RUNNER").ok())
            .unwrap_or_else(|| runners::detect().to_string());
        let runner = runners::build(&runner_name)?;
        // Sessions record the harness that started them, so later turns are
        // resolved per session rather than forced onto this invocation's choice.
        let runner_registry: Arc<dyn RunnerRegistry> =
            Arc::new(CachingRunnerRegistry::new(runner_name.clone()));

        let sessions: Arc<dyn SessionStore> = Arc::new(FsSessionStore::new(&paths));
        let transcript: Arc<dyn TranscriptLog> = Arc::new(FsTranscriptLog::new(paths.clone()));
        let harness_store: Arc<dyn HarnessStore> = Arc::new(FsHarnessStore::new(paths.clone()));
        let goal_store: Arc<dyn GoalStore> = Arc::new(FsGoalStore::new(&paths));
        let heartbeat_store: Arc<dyn HeartbeatStore> = Arc::new(FsHeartbeatStore::new(&paths));
        let schedule_store: Arc<dyn ScheduleStore> = Arc::new(FsScheduleStore::new(&paths));
        let message_store: Arc<dyn MessageStore> = Arc::new(FsMessageStore::new(&paths));
        let registry: Arc<dyn SubagentRegistry> = Arc::new(FsSubagentRegistry::new(&paths));
        let autonomous_store: Arc<dyn AutonomousStore> = Arc::new(FsAutonomousStore::new(&paths));
        let compaction_store: Arc<dyn CompactionStore> = Arc::new(FsCompactionStore::new(&paths));
        let supervisor_port: Arc<dyn ProcessSupervisor> = Arc::new(UnixProcessSupervisor);
        let gates: Arc<dyn GateRunner> = Arc::new(ShellGateRunner::default());
        let catalog: Arc<dyn SkillCatalog> = Arc::new(FsSkillCatalog::default());
        let summariser: Arc<dyn Summariser> = Arc::new(AgentSummariser::new(
            runner.clone(),
            workdir.clone(),
            None,
        ));

        let depth = std::env::var("PA_MAX_DEPTH")
            .ok()
            .and_then(|value| value.parse::<u8>().ok())
            .map(|max_depth| DepthPolicy { max_depth })
            .unwrap_or_default();

        let session_service = SessionService::new(
            env.clone(),
            sessions.clone(),
            transcript.clone(),
            supervisor_port.clone(),
        );
        let goals = Arc::new(GoalService::new(
            env.clone(),
            goal_store,
            sessions.clone(),
            transcript.clone(),
        ));
        let heartbeats = Arc::new(HeartbeatService::new(
            env.clone(),
            heartbeat_store,
            sessions.clone(),
        ));
        let schedules = Arc::new(ScheduleService::new(
            env.clone(),
            schedule_store,
            sessions.clone(),
        ));
        let autonomy = Arc::new(AutonomyService::new(
            env.clone(),
            autonomous_store,
            sessions.clone(),
            gates,
            transcript.clone(),
        ));

        let harness = Arc::new(HarnessService::new(
            env.clone(),
            harness_store.clone(),
            transcript.clone(),
        ));

        Ok(Self {
            session_service,
            review: ReviewService::new(
                sessions.clone(),
                transcript.clone(),
                harness_store,
                runner_registry.clone(),
                harness.clone(),
            ),
            harness,
            subagents: SubagentService::new(
                env.clone(),
                sessions.clone(),
                registry.clone(),
                runner_registry.clone(),
                transcript.clone(),
                depth,
            ),
            messaging: MessagingService::new(
                env.clone(),
                sessions.clone(),
                message_store,
                registry,
                transcript.clone(),
                MessageLimits::default(),
            ),
            compaction: CompactionService::new(
                env.clone(),
                compaction_store,
                sessions.clone(),
                transcript.clone(),
                summariser,
                DEFAULT_CONTEXT_WINDOW,
            ),
            skills: SkillService::new(env.clone(), catalog),
            supervisor: SupervisorService::new(
                env.clone(),
                sessions.clone(),
                runner_registry,
                transcript.clone(),
                heartbeats.clone(),
                schedules.clone(),
                goals.clone(),
                autonomy.clone(),
            ),
            goals,
            heartbeats,
            schedules,
            autonomy,
            sessions,
            transcript,
            runner,
            paths,
            env,
            workdir,
            runner_name,
            session_override: cli.session.clone().or_else(|| std::env::var("PA_SESSION").ok()),
        })
    }

    /// Builds a kernel for a language, defaulting to `$PA_KERNEL` then Python.
    pub fn kernel(&self, language: Option<&str>) -> Result<KernelService> {
        let language = match language {
            Some(name) => KernelLanguage::parse(name)?,
            None => match std::env::var("PA_KERNEL") {
                Ok(name) => KernelLanguage::parse(&name)?,
                Err(_) => KernelLanguage::Python,
            },
        };
        Ok(KernelService::new(
            self.env.clone(),
            pa_infra::kernel::build(language, self.paths.clone(), self.workdir.clone())?,
            self.transcript.clone(),
        ))
    }

    /// The session this invocation acts on, creating it if it does not exist.
    ///
    /// Auto-creation is the difference between a tool an agent can pick up
    /// mid-task and one that demands a setup step first: `pa kernel exec` in a
    /// fresh directory should just work.
    pub fn session(&self) -> Result<Session> {
        if let Some(selector) = &self.session_override {
            return match self.sessions.resolve(selector) {
                Ok(session) => Ok(session),
                Err(DomainError::NotFound { .. }) => self.create(Some(selector.clone())),
                Err(other) => Err(other),
            };
        }

        let name = self.default_session_name();
        match self.sessions.resolve(&name) {
            Ok(session) => Ok(session),
            Err(DomainError::NotFound { .. }) => self.create(Some(name)),
            Err(other) => Err(other),
        }
    }

    /// Resolves a session without creating one — for commands that name a
    /// target, where inventing an agent would hide a typo.
    pub fn resolve(&self, selector: &str) -> Result<Session> {
        self.sessions.resolve(selector)
    }

    pub fn create(&self, name: Option<String>) -> Result<Session> {
        self.session_service.start(StartSession {
            name,
            workdir: self.workdir.clone(),
            runner: self.runner_name.clone(),
            model: std::env::var("PA_MODEL").ok(),
            kind: SessionKind::Root,
            parent: None,
        })
    }

    /// A stable name derived from the working directory, so the same project
    /// keeps the same session across invocations.
    pub fn default_session_name(&self) -> String {
        let base = std::path::Path::new(&self.workdir)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "workspace".to_string());
        Session::normalise_name(&base).unwrap_or_else(|_| "workspace".to_string())
    }
}
