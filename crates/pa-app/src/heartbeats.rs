//! Heartbeats: recurring instructions that re-enter a session.
//!
//! There are two kinds and they are not interchangeable. The user owns exactly
//! one visible heartbeat per session; the agent may create as many internal
//! ones as it needs but can never touch the user's. That asymmetry is the whole
//! design: an agent that could silence the instruction it was given to check in
//! is an agent that can stop reporting.

use std::sync::Arc;

use pa_domain::prelude::*;

use crate::Env;

#[derive(Debug, Clone)]
pub struct CreateHeartbeat {
    pub selector: String,
    pub prompt: String,
    pub interval: Interval,
    pub owner: HeartbeatOwner,
    pub label: Option<String>,
    pub delivery: DeliveryMode,
}

pub struct HeartbeatService {
    env: Env,
    store: Arc<dyn HeartbeatStore>,
    sessions: Arc<dyn SessionStore>,
}

impl std::fmt::Debug for HeartbeatService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HeartbeatService")
    }
}

impl HeartbeatService {
    pub fn new(env: Env, store: Arc<dyn HeartbeatStore>, sessions: Arc<dyn SessionStore>) -> Self {
        Self {
            env,
            store,
            sessions,
        }
    }

    /// Creates a heartbeat. Setting the user's replaces the previous one, since
    /// there is only ever one; an agent's is added alongside the others.
    pub fn create(&self, request: CreateHeartbeat) -> Result<Heartbeat> {
        let session = self.sessions.resolve(&request.selector)?;
        let now = self.env.now();

        if request.owner == HeartbeatOwner::User {
            for existing in self.store.list(Some(&session.id))? {
                if existing.owner == HeartbeatOwner::User {
                    self.store.remove(&existing.id)?;
                }
            }
        }

        let mut heartbeat = Heartbeat::new(
            self.env.id(Ids::HEARTBEAT),
            &session.id,
            request.owner,
            &request.prompt,
            request.interval,
            request.delivery,
            now,
        )?;
        heartbeat.label = request.label;

        self.store.put(&heartbeat)?;
        Ok(heartbeat)
    }

    pub fn list(&self, selector: Option<&str>) -> Result<Vec<Heartbeat>> {
        let session_id = match selector {
            Some(selector) => Some(self.sessions.resolve(selector)?.id),
            None => None,
        };
        let mut rows = self.store.list(session_id.as_deref())?;
        rows.sort_by_key(|row| row.next_fire_at);
        Ok(rows)
    }

    /// The user's single visible heartbeat, if there is one.
    pub fn user_heartbeat(&self, selector: &str) -> Result<Option<Heartbeat>> {
        let session = self.sessions.resolve(selector)?;
        Ok(self
            .store
            .list(Some(&session.id))?
            .into_iter()
            .find(|row| row.owner == HeartbeatOwner::User))
    }

    pub fn pause(&self, id: &str, actor: HeartbeatOwner) -> Result<Heartbeat> {
        self.mutate(id, actor, |heartbeat, _| {
            heartbeat.pause();
        })
    }

    pub fn resume(&self, id: &str, actor: HeartbeatOwner) -> Result<Heartbeat> {
        self.mutate(id, actor, |heartbeat, now| heartbeat.resume(now))
    }

    pub fn remove(&self, id: &str, actor: HeartbeatOwner) -> Result<()> {
        let heartbeat = self.store.get(id)?;
        Self::authorise(&heartbeat, actor)?;
        self.store.remove(id)
    }

    /// Clears the user's heartbeat for a session. Only the user may do this.
    pub fn clear_user(&self, selector: &str) -> Result<bool> {
        match self.user_heartbeat(selector)? {
            Some(heartbeat) => {
                self.store.remove(&heartbeat.id)?;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn due(&self) -> Result<Vec<Heartbeat>> {
        self.store.due(self.env.now())
    }

    /// Marks a heartbeat as fired and schedules the next one.
    pub fn mark_fired(&self, id: &str) -> Result<Heartbeat> {
        let mut heartbeat = self.store.get(id)?;
        heartbeat.mark_fired(self.env.now());
        self.store.put(&heartbeat)?;
        Ok(heartbeat)
    }

    fn mutate<F>(&self, id: &str, actor: HeartbeatOwner, apply: F) -> Result<Heartbeat>
    where
        F: FnOnce(&mut Heartbeat, chrono::DateTime<chrono::Utc>),
    {
        let mut heartbeat = self.store.get(id)?;
        Self::authorise(&heartbeat, actor)?;
        apply(&mut heartbeat, self.env.now());
        self.store.put(&heartbeat)?;
        Ok(heartbeat)
    }

    fn authorise(heartbeat: &Heartbeat, actor: HeartbeatOwner) -> Result<()> {
        if actor == HeartbeatOwner::Agent && !heartbeat.is_agent_writable() {
            return Err(DomainError::forbidden(
                "that heartbeat belongs to the user; the agent cannot change it",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};

    fn fixture() -> (HeartbeatService, Arc<TestClock>) {
        let (env, clock) = env();
        let sessions = Arc::new(MemSessions::default());
        SessionService::new(
            env.clone(),
            sessions.clone(),
            Arc::new(MemTranscript::default()),
            Arc::new(MemSupervisor::default()),
        )
        .start(StartSession {
            name: Some("root".into()),
            workdir: "/work".into(),
            runner: "claude".into(),
            model: None,
            kind: SessionKind::Root,
            parent: None,
        })
        .unwrap();

        (
            HeartbeatService::new(env, Arc::new(MemHeartbeats::default()), sessions),
            clock,
        )
    }

    fn create(service: &HeartbeatService, owner: HeartbeatOwner, every: &str) -> Heartbeat {
        service
            .create(CreateHeartbeat {
                selector: "root".into(),
                prompt: "check the deployment".into(),
                interval: Interval::parse(every).unwrap(),
                owner,
                label: None,
                delivery: DeliveryMode::Auto,
            })
            .unwrap()
    }

    #[test]
    fn setting_the_user_heartbeat_replaces_the_previous_one() {
        let (service, _) = fixture();
        create(&service, HeartbeatOwner::User, "10m");
        create(&service, HeartbeatOwner::User, "5m");
        let all = service.list(Some("root")).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].interval.to_string(), "5m");
    }

    #[test]
    fn agent_heartbeats_accumulate_alongside_the_users() {
        let (service, _) = fixture();
        create(&service, HeartbeatOwner::User, "10m");
        create(&service, HeartbeatOwner::Agent, "5m");
        create(&service, HeartbeatOwner::Agent, "1m");
        assert_eq!(service.list(Some("root")).unwrap().len(), 3);
    }

    #[test]
    fn the_agent_cannot_touch_the_users_heartbeat() {
        let (service, _) = fixture();
        let user = create(&service, HeartbeatOwner::User, "10m");
        assert!(service.pause(&user.id, HeartbeatOwner::Agent).is_err());
        assert!(service.remove(&user.id, HeartbeatOwner::Agent).is_err());
        assert!(service.pause(&user.id, HeartbeatOwner::User).is_ok());
    }

    #[test]
    fn the_user_can_manage_an_agents_heartbeat() {
        let (service, _) = fixture();
        let agent = create(&service, HeartbeatOwner::Agent, "10m");
        assert!(service.remove(&agent.id, HeartbeatOwner::User).is_ok());
    }

    #[test]
    fn due_heartbeats_appear_only_after_their_interval() {
        let (service, clock) = fixture();
        create(&service, HeartbeatOwner::Agent, "10m");
        assert!(service.due().unwrap().is_empty());
        clock.advance_secs(600);
        assert_eq!(service.due().unwrap().len(), 1);
    }

    #[test]
    fn firing_reschedules_from_now_so_a_backlog_cannot_build_up() {
        let (service, clock) = fixture();
        let heartbeat = create(&service, HeartbeatOwner::Agent, "10m");
        clock.advance_secs(3600);
        let fired = service.mark_fired(&heartbeat.id).unwrap();
        assert_eq!(fired.fire_count, 1);
        assert!(service.due().unwrap().is_empty());
    }

    #[test]
    fn listing_is_ordered_by_what_fires_next() {
        let (service, _) = fixture();
        create(&service, HeartbeatOwner::Agent, "10m");
        create(&service, HeartbeatOwner::Agent, "1m");
        let rows = service.list(Some("root")).unwrap();
        assert_eq!(rows[0].interval.to_string(), "1m");
    }
}
