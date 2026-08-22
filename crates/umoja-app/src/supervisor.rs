//! The tick that makes everything else move.
//!
//! Heartbeats, schedules, goals and autonomous continuations are all just
//! "something should re-enter this session now". The supervisor is the one
//! place that decides *what* is due, delivers it through the runner, and folds
//! the resulting cost back into every budget that is watching.
//!
//! It is deliberately a single pass with no sleeping and no threads: `pa tick`
//! is safe to call from cron, from a loop, or by hand.

use std::sync::Arc;

use umoja_domain::prelude::*;
use umoja_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::autonomy::AutonomyService;
use crate::goals::GoalService;
use crate::heartbeats::HeartbeatService;
use crate::schedules::ScheduleService;
use crate::Env;

/// One thing the supervisor did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Delivery {
    pub kind: &'static str,
    pub session: String,
    pub prompt: String,
    pub ok: bool,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickReport {
    pub deliveries: Vec<Delivery>,
}

impl TickReport {
    pub fn is_quiet(&self) -> bool {
        self.deliveries.is_empty()
    }

    pub fn failures(&self) -> usize {
        self.deliveries.iter().filter(|row| !row.ok).count()
    }
}

pub struct SupervisorService {
    env: Env,
    sessions: Arc<dyn SessionStore>,
    runners: Arc<dyn RunnerRegistry>,
    transcript: Arc<dyn TranscriptLog>,
    heartbeats: Arc<HeartbeatService>,
    schedules: Arc<ScheduleService>,
    goals: Arc<GoalService>,
    autonomy: Arc<AutonomyService>,
}

impl std::fmt::Debug for SupervisorService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SupervisorService")
    }
}

impl SupervisorService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        env: Env,
        sessions: Arc<dyn SessionStore>,
        runners: Arc<dyn RunnerRegistry>,
        transcript: Arc<dyn TranscriptLog>,
        heartbeats: Arc<HeartbeatService>,
        schedules: Arc<ScheduleService>,
        goals: Arc<GoalService>,
        autonomy: Arc<AutonomyService>,
    ) -> Self {
        Self {
            env,
            sessions,
            runners,
            transcript,
            heartbeats,
            schedules,
            goals,
            autonomy,
        }
    }

    pub fn tick(&self) -> Result<TickReport> {
        let mut report = TickReport::default();
        self.tick_heartbeats(&mut report)?;
        self.tick_schedules(&mut report)?;
        self.tick_goals(&mut report)?;
        Ok(report)
    }

    fn tick_heartbeats(&self, report: &mut TickReport) -> Result<()> {
        for heartbeat in self.heartbeats.due()? {
            let Ok(session) = self.sessions.get(&heartbeat.session_id) else {
                // The session is gone; the heartbeat has nothing to enter.
                self.heartbeats.remove(&heartbeat.id, HeartbeatOwner::User)?;
                continue;
            };

            // The tick is marked before delivery, not after. A heartbeat that
            // fires, crashes the runner and is retried immediately would beat
            // its own interval; one missed check-in is the cheaper failure.
            self.heartbeats.mark_fired(&heartbeat.id)?;

            let outcome = self.deliver(&session, &heartbeat.prompt, heartbeat.delivery);
            self.transcript.append(&TranscriptRecord::new(
                &session.id,
                self.env.now(),
                TranscriptEvent::HeartbeatFired {
                    heartbeat_id: heartbeat.id.clone(),
                    prompt: heartbeat.prompt.clone(),
                },
            ))?;

            report.deliveries.push(self.record(
                "heartbeat",
                &session,
                &heartbeat.prompt,
                outcome,
            )?);
        }
        Ok(())
    }

    fn tick_schedules(&self, report: &mut TickReport) -> Result<()> {
        for job in self.schedules.due()? {
            let claimed = match self.schedules.claim(&job.id) {
                Ok(claimed) => claimed,
                // Another worker took this tick first. That is the claim doing
                // its job, not an error worth reporting.
                Err(_) => continue,
            };

            let session = match self.sessions.resolve(&claimed.target) {
                Ok(session) => session,
                Err(error) => {
                    self.schedules.fail(&claimed.id, &error.to_string())?;
                    report.deliveries.push(Delivery {
                        kind: "schedule",
                        session: claimed.target.clone(),
                        prompt: claimed.prompt.clone(),
                        ok: false,
                        detail: Some(error.to_string()),
                    });
                    continue;
                }
            };

            let outcome = self.deliver(&session, &claimed.prompt, claimed.delivery);
            match &outcome {
                Ok(run) if run.ok => {
                    self.schedules.complete(&claimed.id)?;
                }
                Ok(run) => {
                    self.schedules
                        .fail(&claimed.id, run.error.as_deref().unwrap_or("run failed"))?;
                }
                Err(error) => {
                    self.schedules.fail(&claimed.id, &error.to_string())?;
                }
            }

            self.transcript.append(&TranscriptRecord::new(
                &session.id,
                self.env.now(),
                TranscriptEvent::ScheduleFired {
                    job_id: claimed.id.clone(),
                    prompt: claimed.prompt.clone(),
                },
            ))?;

            report
                .deliveries
                .push(self.record("schedule", &session, &claimed.prompt, outcome)?);
        }
        Ok(())
    }

    fn tick_goals(&self, report: &mut TickReport) -> Result<()> {
        for goal in self.goals.active()? {
            let Ok(session) = self.sessions.get(&goal.session_id) else {
                continue;
            };

            // Autonomous mode, when it is on, has the final say on whether
            // another continuation is allowed. A goal is an objective; the
            // policy is what decides if there is budget left to pursue it.
            let autonomous = self.autonomy.status(&session.id)?;
            if autonomous.is_some() {
                let step = self.autonomy.step(&session.id)?;
                if !step.decision.should_continue() {
                    continue;
                }
            }

            let Some(prompt) = self.goals.continuation_prompt(&goal) else {
                continue;
            };

            self.goals.record_continuation(&session.id)?;
            let outcome = self.deliver(&session, &prompt, DeliveryMode::FollowUp);
            report
                .deliveries
                .push(self.record("goal", &session, &goal.objective, outcome)?);
        }
        Ok(())
    }

    fn deliver(
        &self,
        session: &Session,
        prompt: &str,
        delivery: DeliveryMode,
    ) -> Result<RunOutcome> {
        let mut request = RunRequest::new(&session.id, prompt, &session.workdir)?
            .with_model(session.model.clone());
        request.delivery = delivery;
        // The session's own harness, not whichever one this tick was invoked
        // with: a conversation started under one runner cannot be continued
        // under another.
        self.runners.get(&session.runner)?.run(&request)
    }

    /// Folds a delivery's cost into everything that is counting.
    fn record(
        &self,
        kind: &'static str,
        session: &Session,
        prompt: &str,
        outcome: Result<RunOutcome>,
    ) -> Result<Delivery> {
        match outcome {
            Ok(run) => {
                if run.ok {
                    let mut updated = self.sessions.get(&session.id)?;
                    updated.usage.absorb(&run.usage);
                    updated.updated_at = self.env.now();
                    self.sessions.update(&updated)?;

                    self.goals.observe(&session.id, &run.usage)?;
                    self.autonomy.record_turn(&session.id, &run.usage)?;
                }
                Ok(Delivery {
                    kind,
                    session: session.name.clone(),
                    prompt: prompt.to_string(),
                    ok: run.ok,
                    detail: run.error,
                })
            }
            Err(error) => {
                self.transcript.append(&TranscriptRecord::new(
                    &session.id,
                    self.env.now(),
                    TranscriptEvent::Error {
                        context: kind.to_string(),
                        detail: error.to_string(),
                    },
                ))?;
                Ok(Delivery {
                    kind,
                    session: session.name.clone(),
                    prompt: prompt.to_string(),
                    ok: false,
                    detail: Some(error.to_string()),
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::autonomy::AutonomyService;
    use crate::doubles::*;
    use crate::goals::GoalService;
    use crate::heartbeats::{CreateHeartbeat, HeartbeatService};
    use crate::schedules::{AddJob, ScheduleService};
    use crate::sessions::{SessionService, StartSession};

    struct Fixture {
        runners: Arc<MemRunnerRegistry>,
        supervisor: SupervisorService,
        heartbeats: Arc<HeartbeatService>,
        schedules: Arc<ScheduleService>,
        goals: Arc<GoalService>,
        autonomy: Arc<AutonomyService>,
        runner: Arc<MemRunner>,
        clock: Arc<TestClock>,
        sessions: Arc<MemSessions>,
    }

    fn fixture() -> Fixture {
        let (env, clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let transcript = Arc::new(MemTranscript::default());
        let runner = Arc::new(MemRunner::ready());
        let runners = Arc::new(MemRunnerRegistry::new(runner.clone()));

        SessionService::new(
            env.clone(),
            sessions.clone(),
            transcript.clone(),
            Arc::new(MemSupervisor::default()),
        )
        .start(StartSession {
            name: Some("worker".into()),
            workdir: "/work".into(),
            runner: "claude".into(),
            model: None,
            kind: SessionKind::Root,
            parent: None,
        })
        .unwrap();

        let heartbeats = Arc::new(HeartbeatService::new(
            env.clone(),
            Arc::new(MemHeartbeats::default()),
            sessions.clone(),
        ));
        let schedules = Arc::new(ScheduleService::new(
            env.clone(),
            Arc::new(MemSchedules::default()),
            sessions.clone(),
        ));
        let goals = Arc::new(GoalService::new(
            env.clone(),
            Arc::new(MemGoals::default()),
            sessions.clone(),
            transcript.clone(),
        ));
        let autonomy = Arc::new(AutonomyService::new(
            env.clone(),
            Arc::new(MemAutonomous::default()),
            sessions.clone(),
            Arc::new(MemGates::default()),
            transcript.clone(),
        ));

        Fixture {
            runners: runners.clone(),
            supervisor: SupervisorService::new(
                env,
                sessions.clone(),
                runners.clone(),
                transcript,
                heartbeats.clone(),
                schedules.clone(),
                goals.clone(),
                autonomy.clone(),
            ),
            heartbeats,
            schedules,
            goals,
            autonomy,
            runner,
            clock,
            sessions,
        }
    }

    #[test]
    fn a_quiet_tick_does_nothing_at_all() {
        let fixture = fixture();
        assert!(fixture.supervisor.tick().unwrap().is_quiet());
        assert!(fixture.runner.prompts().is_empty());
    }

    #[test]
    fn a_due_heartbeat_is_delivered_once_per_interval() {
        let fixture = fixture();
        fixture
            .heartbeats
            .create(CreateHeartbeat {
                selector: "worker".into(),
                prompt: "check the deployment".into(),
                interval: Interval::parse("10m").unwrap(),
                owner: HeartbeatOwner::User,
                label: None,
                delivery: DeliveryMode::Auto,
            })
            .unwrap();

        fixture.clock.advance_secs(600);
        let report = fixture.supervisor.tick().unwrap();
        assert_eq!(report.deliveries.len(), 1);
        assert_eq!(report.deliveries[0].kind, "heartbeat");

        // Immediately ticking again must not fire it a second time.
        assert!(fixture.supervisor.tick().unwrap().is_quiet());
    }

    #[test]
    fn a_due_schedule_runs_and_a_one_time_job_retires() {
        let fixture = fixture();
        let job = fixture
            .schedules
            .add(AddJob {
                target: "worker".into(),
                when: "in 30m".into(),
                prompt: "check the benchmark".into(),
                delivery: DeliveryMode::Auto,
            })
            .unwrap();

        fixture.clock.advance_secs(1800);
        let report = fixture.supervisor.tick().unwrap();
        assert_eq!(report.deliveries.len(), 1);
        assert!(report.deliveries[0].ok);
        assert_eq!(
            fixture.schedules.get(&job.id).unwrap().status,
            JobStatus::Completed
        );
    }

    #[test]
    fn a_schedule_whose_agent_vanished_fails_rather_than_hanging() {
        let fixture = fixture();
        let job = fixture
            .schedules
            .add(AddJob {
                target: "worker".into(),
                when: "in 1m".into(),
                prompt: "go".into(),
                delivery: DeliveryMode::Auto,
            })
            .unwrap();
        let session = fixture.sessions.resolve("worker").unwrap();
        fixture.sessions.remove(&session.id).unwrap();

        fixture.clock.advance_secs(60);
        let report = fixture.supervisor.tick().unwrap();
        assert_eq!(report.failures(), 1);
        assert_eq!(fixture.schedules.get(&job.id).unwrap().status, JobStatus::Failed);
    }

    #[test]
    fn an_active_goal_is_continued_and_its_cost_counted() {
        let fixture = fixture();
        fixture
            .goals
            .create(
                "worker",
                "ship the release",
                GoalBudget {
                    tokens: Some(10_000),
                    ..Default::default()
                },
                false,
            )
            .unwrap();

        let report = fixture.supervisor.tick().unwrap();
        assert_eq!(report.deliveries.len(), 1);
        assert_eq!(report.deliveries[0].kind, "goal");
        assert!(fixture.runner.prompts()[0].contains("ship the release"));

        let goal = fixture.goals.require("worker").unwrap();
        assert_eq!(goal.progress.continuations, 1);
        assert_eq!(goal.progress.tokens_used, 15);
    }

    #[test]
    fn a_goal_that_runs_out_of_budget_stops_being_continued() {
        let fixture = fixture();
        fixture
            .goals
            .create(
                "worker",
                "ship the release",
                // One delivered turn costs 15 tokens, so this budget cannot
                // survive the first continuation.
                GoalBudget {
                    tokens: Some(10),
                    ..Default::default()
                },
                false,
            )
            .unwrap();

        fixture.supervisor.tick().unwrap();
        let goal = fixture.goals.require("worker").unwrap();
        assert_eq!(goal.status, GoalStatus::BudgetExhausted);

        assert!(fixture.supervisor.tick().unwrap().is_quiet());
    }

    #[test]
    fn a_paused_goal_is_left_alone() {
        let fixture = fixture();
        fixture
            .goals
            .create("worker", "ship it", GoalBudget::default(), false)
            .unwrap();
        fixture.goals.pause("worker").unwrap();
        assert!(fixture.supervisor.tick().unwrap().is_quiet());
    }

    #[test]
    fn autonomous_limits_override_an_active_goal() {
        let fixture = fixture();
        fixture
            .goals
            .create("worker", "ship it", GoalBudget::default(), false)
            .unwrap();

        let mut policy = AutonomousPolicy {
            enabled: true,
            gates: vec![],
            limits: AutonomousLimits::default(),
        };
        policy.limits.max_continuations = 0;
        fixture.autonomy.enable("worker", policy).unwrap();

        // The goal is still active, but the policy has no continuations left.
        assert!(fixture.supervisor.tick().unwrap().is_quiet());
        assert_eq!(
            fixture.goals.require("worker").unwrap().progress.continuations,
            0
        );
    }

    #[test]
    fn a_session_is_continued_on_the_harness_that_started_it() {
        let fixture = fixture();

        // The session was started under "claude"; this tick must ask for that
        // harness, not for whichever one the tick itself was configured with.
        let mut session = fixture.sessions.resolve("worker").unwrap();
        session.runner = "opencode".into();
        fixture.sessions.update(&session).unwrap();

        fixture
            .goals
            .create("worker", "ship it", GoalBudget::default(), false)
            .unwrap();
        fixture.supervisor.tick().unwrap();

        assert_eq!(
            *fixture.runners.asked.lock().unwrap(),
            vec!["opencode".to_string()]
        );
    }

    #[test]
    fn a_runner_that_is_missing_reports_instead_of_pretending() {
        let fixture = fixture();
        fixture
            .goals
            .create("worker", "ship it", GoalBudget::default(), false)
            .unwrap();
        *fixture.runner.available.lock().unwrap() = false;

        let report = fixture.supervisor.tick().unwrap();
        assert_eq!(report.failures(), 1);
        assert!(report.deliveries[0]
            .detail
            .as_ref()
            .unwrap()
            .contains("not installed"));
    }
}
