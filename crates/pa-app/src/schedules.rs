//! One-time and cron schedules aimed at an agent.

use std::sync::Arc;

use pa_domain::prelude::*;

use crate::Env;

#[derive(Debug, Clone)]
pub struct AddJob {
    pub target: String,
    /// `in 30m`, an RFC 3339 instant, or a five-field cron expression.
    pub when: String,
    pub prompt: String,
    pub delivery: DeliveryMode,
}

pub struct ScheduleService {
    env: Env,
    store: Arc<dyn ScheduleStore>,
    sessions: Arc<dyn SessionStore>,
}

impl std::fmt::Debug for ScheduleService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ScheduleService")
    }
}

impl ScheduleService {
    pub fn new(env: Env, store: Arc<dyn ScheduleStore>, sessions: Arc<dyn SessionStore>) -> Self {
        Self {
            env,
            store,
            sessions,
        }
    }

    pub fn add(&self, request: AddJob) -> Result<ScheduledJob> {
        let now = self.env.now();
        // The target is resolved now so a typo fails at the prompt rather than
        // silently at 3am, but stored by *name* so a later rename still works.
        let session = self.sessions.resolve(&request.target)?;
        let spec = ScheduleSpec::parse(&request.when, now)?;

        let job = ScheduledJob::new(
            self.env.id(Ids::JOB),
            &session.name,
            &request.prompt,
            spec,
            request.delivery,
            now,
        )?;

        self.store.put(&job)?;
        Ok(job)
    }

    pub fn list(&self, target: Option<&str>, include_finished: bool) -> Result<Vec<ScheduledJob>> {
        let name = match target {
            Some(selector) => Some(self.sessions.resolve(selector)?.name),
            None => None,
        };
        let mut rows = self.store.list(name.as_deref(), include_finished)?;
        rows.sort_by(|a, b| match (a.next_tick, b.next_tick) {
            (Some(a), Some(b)) => a.cmp(&b),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => std::cmp::Ordering::Equal,
        });
        Ok(rows)
    }

    pub fn get(&self, id: &str) -> Result<ScheduledJob> {
        self.store.get(id)
    }

    pub fn cancel(&self, id: &str) -> Result<ScheduledJob> {
        let mut job = self.store.get(id)?;
        job.cancel();
        self.store.put(&job)?;
        Ok(job)
    }

    pub fn due(&self) -> Result<Vec<ScheduledJob>> {
        self.store.due(self.env.now())
    }

    /// Takes a due tick before delivering it.
    ///
    /// Claim-then-deliver is why a crash mid-delivery does not replay an
    /// uncertain prompt: the tick is already spent when the process dies.
    pub fn claim(&self, id: &str) -> Result<ScheduledJob> {
        let mut job = self.store.get(id)?;
        job.claim(self.env.now())?;
        self.store.put(&job)?;
        Ok(job)
    }

    pub fn complete(&self, id: &str) -> Result<ScheduledJob> {
        let mut job = self.store.get(id)?;
        job.complete_tick(self.env.now());
        self.store.put(&job)?;
        Ok(job)
    }

    pub fn fail(&self, id: &str, reason: &str) -> Result<ScheduledJob> {
        let mut job = self.store.get(id)?;
        job.fail_tick(reason, self.env.now());
        self.store.put(&job)?;
        Ok(job)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};

    fn fixture() -> (ScheduleService, Arc<TestClock>, Arc<MemSessions>, SessionService) {
        let (env, clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let session_service = SessionService::new(
            env.clone(),
            sessions.clone(),
            Arc::new(MemTranscript::default()),
            Arc::new(MemSupervisor::default()),
        );
        session_service
            .start(StartSession {
                name: Some("worker".into()),
                workdir: "/work".into(),
                runner: "claude".into(),
                model: None,
                kind: SessionKind::Root,
                parent: None,
            })
            .unwrap();
        (
            ScheduleService::new(env, Arc::new(MemSchedules::default()), sessions.clone()),
            clock,
            sessions,
            session_service,
        )
    }

    fn add(service: &ScheduleService, when: &str) -> Result<ScheduledJob> {
        service.add(AddJob {
            target: "worker".into(),
            when: when.into(),
            prompt: "check the benchmark".into(),
            delivery: DeliveryMode::Auto,
        })
    }

    #[test]
    fn a_relative_job_becomes_due_when_the_clock_reaches_it() {
        let (service, clock, _, _) = fixture();
        add(&service, "in 30m").unwrap();
        assert!(service.due().unwrap().is_empty());
        clock.advance_secs(1800);
        assert_eq!(service.due().unwrap().len(), 1);
    }

    #[test]
    fn an_unknown_target_fails_at_the_prompt() {
        let (service, _, _, _) = fixture();
        assert!(service
            .add(AddJob {
                target: "nobody".into(),
                when: "in 5m".into(),
                prompt: "go".into(),
                delivery: DeliveryMode::Auto,
            })
            .is_err());
    }

    #[test]
    fn a_renamed_agent_keeps_its_schedule() {
        let (service, clock, _, session_service) = fixture();
        add(&service, "in 5m").unwrap();
        session_service.rename("worker", "builder").unwrap();
        clock.advance_secs(300);

        let due = service.due().unwrap();
        assert_eq!(due.len(), 1);
        // The job still names the agent it was created for; resolution happens
        // at delivery, so the rename is not a lost schedule.
        assert_eq!(due[0].target, "worker");
    }

    #[test]
    fn a_cron_job_reschedules_and_a_one_time_job_retires() {
        let (service, clock, _, _) = fixture();
        let once = add(&service, "in 10m").unwrap();
        let cron = add(&service, "*/15 * * * *").unwrap();

        clock.set(at("2026-08-16T12:15:00Z"));
        service.claim(&once.id).unwrap();
        let once = service.complete(&once.id).unwrap();
        assert_eq!(once.status, JobStatus::Completed);

        service.claim(&cron.id).unwrap();
        let cron = service.complete(&cron.id).unwrap();
        assert_eq!(cron.status, JobStatus::Pending);
        assert_eq!(cron.next_tick, Some(at("2026-08-16T12:30:00Z")));
    }

    #[test]
    fn a_failed_delivery_retries_a_cron_job_and_retires_a_one_time_one() {
        let (service, clock, _, _) = fixture();
        let once = add(&service, "in 10m").unwrap();
        let cron = add(&service, "*/15 * * * *").unwrap();
        clock.set(at("2026-08-16T12:15:00Z"));

        service.claim(&once.id).unwrap();
        assert_eq!(service.fail(&once.id, "worker gone").unwrap().status, JobStatus::Failed);

        service.claim(&cron.id).unwrap();
        assert_eq!(service.fail(&cron.id, "worker gone").unwrap().status, JobStatus::Pending);
    }

    #[test]
    fn a_cancelled_job_never_fires_again() {
        let (service, clock, _, _) = fixture();
        let job = add(&service, "*/1 * * * *").unwrap();
        service.cancel(&job.id).unwrap();
        clock.advance_secs(3600);
        assert!(service.due().unwrap().is_empty());
    }

    #[test]
    fn listing_hides_finished_jobs_unless_asked() {
        let (service, clock, _, _) = fixture();
        let job = add(&service, "in 1m").unwrap();
        clock.advance_secs(60);
        service.claim(&job.id).unwrap();
        service.complete(&job.id).unwrap();

        assert!(service.list(Some("worker"), false).unwrap().is_empty());
        assert_eq!(service.list(Some("worker"), true).unwrap().len(), 1);
    }
}
