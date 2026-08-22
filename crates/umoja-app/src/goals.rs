use std::sync::Arc;

use umoja_domain::prelude::*;
use umoja_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

#[derive(Clone)]
pub struct GoalService {
    env: Env,
    goals: Arc<dyn GoalStore>,
    sessions: Arc<dyn SessionStore>,
    transcript: Arc<dyn TranscriptLog>,
}

impl GoalService {
    pub fn new(
        env: Env,
        goals: Arc<dyn GoalStore>,
        sessions: Arc<dyn SessionStore>,
        transcript: Arc<dyn TranscriptLog>,
    ) -> Self {
        Self {
            env,
            goals,
            sessions,
            transcript,
        }
    }

    pub fn create(
        &self,
        selector: &str,
        objective: &str,
        budget: GoalBudget,
        overwrite: bool,
    ) -> Result<Goal> {
        let session = self.sessions.resolve(selector)?;
        let now = self.env.now();

        if let Some(existing) = self.goals.get(&session.id)? {
            if existing.status == GoalStatus::Active && !overwrite {
                return Err(DomainError::conflict("goal", &session.id));
            }
        }

        let goal = Goal::new(&session.id, objective, budget, now)?;
        self.goals.put(&goal)?;
        self.log(&session.id, &goal, now)?;
        Ok(goal)
    }

    pub fn get(&self, selector: &str) -> Result<Option<Goal>> {
        let session = self.sessions.resolve(selector)?;
        self.goals.get(&session.id)
    }

    pub fn require(&self, selector: &str) -> Result<Goal> {
        self.get(selector)?
            .ok_or_else(|| DomainError::not_found("goal", selector))
    }

    pub fn pause(&self, selector: &str) -> Result<Goal> {
        self.mutate(selector, |goal, now| goal.pause(now))
    }

    pub fn resume(&self, selector: &str) -> Result<Goal> {
        self.mutate(selector, |goal, now| goal.resume(now))
    }

    /// The only path to success.
    pub fn complete(&self, selector: &str) -> Result<Goal> {
        self.mutate(selector, |goal, now| goal.complete(now))
    }

    pub fn clear(&self, selector: &str) -> Result<()> {
        let session = self.sessions.resolve(selector)?;
        let now = self.env.now();
        if let Some(mut goal) = self.goals.get(&session.id)? {
            goal.clear(now);
            self.log(&session.id, &goal, now)?;
        }
        self.goals.clear(&session.id)
    }

    // -------------------------------------------------------------------------
    // Checklist Operations
    // -------------------------------------------------------------------------

    pub fn add_step(&self, selector: &str, text: &str) -> Result<(Goal, u32)> {
        let mut new_id = 0;
        let goal = self.mutate(selector, |goal, now| {
            new_id = goal.add_step(text, now)?;
            Ok(())
        })?;
        Ok((goal, new_id))
    }

    pub fn check_step(&self, selector: &str, id: u32) -> Result<Goal> {
        self.mutate(selector, |goal, now| {
            goal.check_step(id, now)?;
            Ok(())
        })
    }

    pub fn uncheck_step(&self, selector: &str, id: u32) -> Result<Goal> {
        self.mutate(selector, |goal, now| {
            goal.uncheck_step(id, now)?;
            Ok(())
        })
    }

    pub fn remove_step(&self, selector: &str, id: u32) -> Result<Goal> {
        self.mutate(selector, |goal, now| {
            goal.remove_step(id, now)?;
            Ok(())
        })
    }

    /// Folds a turn's cost into the goal and reports whether that ended it.
    pub fn observe(&self, selector: &str, usage: &Usage) -> Result<Option<Goal>> {
        let session = self.sessions.resolve(selector)?;
        let Some(mut goal) = self.goals.get(&session.id)? else {
            return Ok(None);
        };
        let now = self.env.now();
        let exhausted = goal.observe(usage, now);
        self.goals.put(&goal)?;
        if exhausted {
            self.log(&session.id, &goal, now)?;
        }
        Ok(Some(goal))
    }

    /// The text put in front of the model when a goal is still open.
    pub fn continuation_prompt(&self, goal: &Goal) -> Option<String> {
        if !goal.should_continue() {
            return None;
        }
        let mut prompt = format!("<goal status=\"{}\">\n{}\n", goal.status.label(), goal.objective);
        if !goal.checklist.is_empty() {
            prompt.push_str(&format!("Checklist: {}\n", goal.progress_summary()));
            if let Some(next) = goal.next_step() {
                prompt.push_str(&format!("Next Step: [ ] {}. {}\n", next.id, next.text));
            }
        }
        if let Some(remaining) = goal.remaining_tokens() {
            prompt.push_str(&format!(
                "Budget: {remaining} of {} tokens left.\n",
                goal.budget.tokens.unwrap_or_default()
            ));
        }
        prompt.push_str(
            "Continue until this is done. When it is genuinely complete, \
             say so by completing the goal — do not stop early and do not \
             claim completion you cannot show.\n</goal>",
        );
        Some(prompt)
    }

    /// Records that a continuation was issued.
    pub fn record_continuation(&self, selector: &str) -> Result<Goal> {
        self.mutate(selector, |goal, now| {
            goal.record_continuation(now);
            Ok(())
        })
    }

    pub fn active(&self) -> Result<Vec<Goal>> {
        self.goals.active()
    }

    fn mutate<F>(&self, selector: &str, apply: F) -> Result<Goal>
    where
        F: FnOnce(&mut Goal, chrono::DateTime<chrono::Utc>) -> Result<()>,
    {
        let session = self.sessions.resolve(selector)?;
        let now = self.env.now();
        let mut goal = self
            .goals
            .get(&session.id)?
            .ok_or_else(|| DomainError::not_found("goal", selector))?;
        apply(&mut goal, now)?;
        self.goals.put(&goal)?;
        self.log(&session.id, &goal, now)?;
        Ok(goal)
    }

    fn log(
        &self,
        session_id: &str,
        goal: &Goal,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<()> {
        self.transcript.append(&TranscriptRecord::new(
            session_id,
            now,
            TranscriptEvent::GoalChanged {
                status: goal.status.label().to_string(),
                objective: goal.objective.clone(),
            },
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};

    fn fixture() -> (GoalService, Arc<TestClock>) {
        let (env, clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let transcript = Arc::new(MemTranscript::default());
        let session_service = SessionService::new(
            env.clone(),
            sessions.clone(),
            transcript.clone(),
            Arc::new(MemSupervisor::default()),
        );
        session_service
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
            GoalService::new(env, Arc::new(MemGoals::default()), sessions, transcript),
            clock,
        )
    }

    fn create(service: &GoalService, tokens: Option<u64>) -> Result<Goal> {
        service.create(
            "root",
            "ship the release",
            GoalBudget {
                tokens,
                ..Default::default()
            },
            false,
        )
    }

    #[test]
    fn a_second_goal_needs_permission_to_replace_the_first() {
        let (service, _) = fixture();
        create(&service, None).unwrap();
        assert!(create(&service, None).is_err());
        assert!(service
            .create("root", "different", GoalBudget::default(), true)
            .is_ok());
    }

    #[test]
    fn checklist_steps_can_be_added_checked_and_uncheck() {
        let (service, _) = fixture();
        create(&service, None).unwrap();
        let (_, id1) = service.add_step("root", "Step 1: Write model").unwrap();
        let (_, id2) = service.add_step("root", "Step 2: Wire CLI").unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        let goal = service.check_step("root", id1).unwrap();
        assert_eq!(goal.progress_summary(), "1/2 steps complete (50%)");

        let prompt = service.continuation_prompt(&goal).unwrap();
        assert!(prompt.contains("1/2 steps complete (50%)"));
        assert!(prompt.contains("Next Step: [ ] 2. Step 2: Wire CLI"));
    }
}
