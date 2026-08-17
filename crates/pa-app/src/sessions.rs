//! Starting, naming, listing and stopping agent sessions.

use std::sync::Arc;

use pa_domain::prelude::*;
use pa_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

#[derive(Debug, Clone)]
pub struct StartSession {
    pub name: Option<String>,
    pub workdir: String,
    pub runner: String,
    pub model: Option<String>,
    pub kind: SessionKind,
    pub parent: Option<Session>,
}

pub struct SessionService {
    env: Env,
    sessions: Arc<dyn SessionStore>,
    transcript: Arc<dyn TranscriptLog>,
    supervisor: Arc<dyn ProcessSupervisor>,
}

impl std::fmt::Debug for SessionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionService")
    }
}

impl SessionService {
    pub fn new(
        env: Env,
        sessions: Arc<dyn SessionStore>,
        transcript: Arc<dyn TranscriptLog>,
        supervisor: Arc<dyn ProcessSupervisor>,
    ) -> Self {
        Self {
            env,
            sessions,
            transcript,
            supervisor,
        }
    }

    pub fn start(&self, request: StartSession) -> Result<Session> {
        let now = self.env.now();
        let id = self.env.id(Ids::SESSION);

        let name = match request.name {
            Some(raw) => Session::normalise_name(&raw)?,
            // An unnamed session still needs an address, so it gets one derived
            // from its id rather than being left un-addressable.
            None => Session::normalise_name(&id)?,
        };

        if self.sessions.resolve(&name).is_ok() {
            return Err(DomainError::conflict("session name", name));
        }

        let depth = request.parent.as_ref().map_or(0, |parent| parent.depth + 1);

        let session = Session {
            id: id.clone(),
            name: name.clone(),
            kind: request.kind,
            status: SessionStatus::Idle,
            workdir: request.workdir.clone(),
            runner: request.runner.clone(),
            model: request.model.clone(),
            parent_id: request.parent.as_ref().map(|parent| parent.id.clone()),
            depth,
            created_at: now,
            updated_at: now,
            usage: Usage::default(),
            pid: None,
        };

        self.sessions.insert(&session)?;
        self.transcript.append(&TranscriptRecord::new(
            &session.id,
            now,
            TranscriptEvent::SessionStarted {
                name,
                runner: request.runner,
                workdir: request.workdir,
                model: request.model,
            },
        ))?;

        Ok(session)
    }

    pub fn resolve(&self, selector: &str) -> Result<Session> {
        self.sessions.resolve(selector)
    }

    /// Live sessions first, then the rest, newest first within each group —
    /// the order somebody scanning `pa agents` actually wants.
    pub fn list(&self, include_finished: bool) -> Result<Vec<Session>> {
        let mut rows = self.sessions.list()?;
        rows.retain(|row| include_finished || row.status.is_live());
        rows.sort_by(|a, b| {
            b.status
                .is_live()
                .cmp(&a.status.is_live())
                .then_with(|| b.updated_at.cmp(&a.updated_at))
        });
        Ok(rows)
    }

    pub fn rename(&self, selector: &str, new_name: &str) -> Result<Session> {
        let mut session = self.sessions.resolve(selector)?;
        let name = Session::normalise_name(new_name)?;
        if name != session.name && self.sessions.resolve(&name).is_ok() {
            return Err(DomainError::conflict("session name", name));
        }
        session.name = name;
        session.updated_at = self.env.now();
        self.sessions.update(&session)?;
        Ok(session)
    }

    /// Stops a session's worker, if it has one, and marks it cancelled.
    pub fn stop(&self, selector: &str, force: bool) -> Result<Session> {
        let mut session = self.sessions.resolve(selector)?;
        if let Some(pid) = session.pid {
            if self.supervisor.is_alive(pid) {
                self.supervisor.terminate(pid, force)?;
            }
        }
        session.pid = None;
        session.status = SessionStatus::Cancelled;
        session.updated_at = self.env.now();
        self.sessions.update(&session)?;
        self.transcript.append(&TranscriptRecord::new(
            &session.id,
            session.updated_at,
            TranscriptEvent::SessionEnded {
                status: session.status.label().to_string(),
            },
        ))?;
        Ok(session)
    }

    pub fn settle(&self, session_id: &str, status: SessionStatus) -> Result<Session> {
        let mut session = self.sessions.get(session_id)?;
        session.status = status;
        session.updated_at = self.env.now();
        self.sessions.update(&session)?;
        self.transcript.append(&TranscriptRecord::new(
            session_id,
            session.updated_at,
            TranscriptEvent::SessionEnded {
                status: status.label().to_string(),
            },
        ))?;
        Ok(session)
    }

    /// Folds one turn's usage into the session.
    pub fn record_usage(&self, session_id: &str, usage: &Usage) -> Result<Session> {
        let mut session = self.sessions.get(session_id)?;
        session.usage.absorb(usage);
        session.updated_at = self.env.now();
        self.sessions.update(&session)?;
        Ok(session)
    }

    /// Reconciles what the registry believes against what is actually running.
    ///
    /// This is `pa doctor`: a session whose worker died while it was `running`
    /// is the normal way state goes stale, and quietly correcting it beats
    /// reporting work that is not happening.
    pub fn reconcile(&self) -> Result<Vec<(String, String)>> {
        let mut fixes = Vec::new();
        for mut session in self.sessions.list()? {
            let Some(pid) = session.pid else {
                if session.status == SessionStatus::Running {
                    session.status = SessionStatus::Idle;
                    session.updated_at = self.env.now();
                    self.sessions.update(&session)?;
                    fixes.push((
                        session.name.clone(),
                        "was marked running with no worker; now idle".to_string(),
                    ));
                }
                continue;
            };
            if !self.supervisor.is_alive(pid) {
                session.pid = None;
                session.status = SessionStatus::Failed;
                session.updated_at = self.env.now();
                self.sessions.update(&session)?;
                fixes.push((
                    session.name.clone(),
                    format!("worker {pid} is gone; marked failed"),
                ));
            }
        }
        Ok(fixes)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;

    fn service() -> (SessionService, Arc<MemSessions>, Arc<MemSupervisor>) {
        let (env, _clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let supervisor = Arc::new(MemSupervisor::default());
        let service = SessionService::new(
            env,
            sessions.clone(),
            Arc::new(MemTranscript::default()),
            supervisor.clone(),
        );
        (service, sessions, supervisor)
    }

    fn start(service: &SessionService, name: Option<&str>) -> Result<Session> {
        service.start(StartSession {
            name: name.map(str::to_string),
            workdir: "/work".into(),
            runner: "claude".into(),
            model: None,
            kind: SessionKind::Root,
            parent: None,
        })
    }

    #[test]
    fn a_started_session_is_addressable_by_name() {
        let (service, _, _) = service();
        let session = start(&service, Some("API Reviewer")).unwrap();
        assert_eq!(session.name, "api-reviewer");
        assert_eq!(service.resolve("api-reviewer").unwrap().id, session.id);
        assert_eq!(service.resolve(&session.id).unwrap().id, session.id);
    }

    #[test]
    fn an_unnamed_session_still_gets_an_address() {
        let (service, _, _) = service();
        let session = start(&service, None).unwrap();
        assert!(!session.name.is_empty());
        assert!(service.resolve(&session.name).is_ok());
    }

    #[test]
    fn names_cannot_collide() {
        let (service, _, _) = service();
        start(&service, Some("worker")).unwrap();
        assert!(matches!(
            start(&service, Some("worker")),
            Err(DomainError::Conflict { .. })
        ));
    }

    #[test]
    fn renaming_onto_a_taken_name_is_refused() {
        let (service, _, _) = service();
        start(&service, Some("one")).unwrap();
        start(&service, Some("two")).unwrap();
        assert!(service.rename("two", "one").is_err());
        assert_eq!(service.rename("two", "Two Point Oh").unwrap().name, "two-point-oh");
    }

    #[test]
    fn stopping_terminates_the_worker() {
        let (service, sessions, supervisor) = service();
        let mut session = start(&service, Some("worker")).unwrap();
        session.pid = Some(99);
        session.status = SessionStatus::Running;
        sessions.update(&session).unwrap();
        supervisor.alive.lock().unwrap().push(99);

        let stopped = service.stop("worker", false).unwrap();
        assert_eq!(stopped.status, SessionStatus::Cancelled);
        assert!(stopped.pid.is_none());
        assert_eq!(*supervisor.terminated.lock().unwrap(), vec![99]);
    }

    #[test]
    fn doctor_notices_a_worker_that_died() {
        let (service, sessions, _) = service();
        let mut session = start(&service, Some("worker")).unwrap();
        session.pid = Some(1234);
        session.status = SessionStatus::Running;
        sessions.update(&session).unwrap();

        let fixes = service.reconcile().unwrap();
        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].1.contains("1234"));
        assert_eq!(
            service.resolve("worker").unwrap().status,
            SessionStatus::Failed
        );
    }

    #[test]
    fn listing_puts_live_sessions_first() {
        let (service, _, _) = service();
        let done = start(&service, Some("done")).unwrap();
        start(&service, Some("live")).unwrap();
        service.settle(&done.id, SessionStatus::Completed).unwrap();

        let all = service.list(true).unwrap();
        assert_eq!(all[0].name, "live");
        let live_only = service.list(false).unwrap();
        assert_eq!(live_only.len(), 1);
    }
}
