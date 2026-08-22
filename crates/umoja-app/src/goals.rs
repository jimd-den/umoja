//! Persistent goals.
//!
//! A goal is an explicit act. Nothing here infers one from a task, because a
//! harness that quietly decides it now has a long-running objective is a
//! harness that keeps spending after you thought it stopped.

use std::sync::Arc;

use umoja_domain::prelude::*;
use umoja_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

pub struct GoalService {
    env: Env,
    goals: Arc<dyn GoalStore>,
    sessions: Arc<dyn SessionStore>,
    transcript: Arc<dyn TranscriptLog>,
}

impl std::fmt::Debug for GoalService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GoalService")
    }
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
        replace: bool,
    ) -> Result<Goal> {
        let session = self.sessions.resolve(selector)?;
        let now = self.env.now();

        if let Some(existing) = self.goals.get(&session.id)? {
            if !existing.status.is_terminal() && !replace {
                return Err(DomainError::forbidden(format!(
                    "'{}' already has a {} goal; clear it or pass replace",
                    session.name,
                    existing.status.label()
                )));
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
    ///
    /// It restates the objective and what has been spent, and — deliberately —
    /// says how to finish. A goal the model cannot see how to close is a goal
    /// that runs until the budget does.
    pub fn continuation_prompt(&self, goal: &Goal) -> Option<String> {
        if !goal.should_continue() {
            return None;
        }
        let mut prompt = format!("<goal status=\"{}\">\n{}\n", goal.status.label(), goal.objective);
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
    fn a_completed_goal_leaves_room_for_the_next_one() {
        let (service, _) = fixture();
        create(&service, None).unwrap();
        service.complete("root").unwrap();
        assert!(create(&service, None).is_ok());
    }

    #[test]
    fn spending_the_budget_stops_the_goal_without_claiming_success() {
        let (service, _) = fixture();
        create(&service, Some(100)).unwrap();
        let goal = service
            .observe(
                "root",
                &Usage {
                    input_tokens: 90,
                    output_tokens: 30,
                    turns: 1,
                    attributed_child_tokens: 0,
                },
            )
            .unwrap()
            .unwrap();

        assert_eq!(goal.status, GoalStatus::BudgetExhausted);
        assert!(service.continuation_prompt(&goal).is_none());
        assert!(service.complete("root").is_err());
    }

    #[test]
    fn a_paused_goal_produces_no_continuation() {
        let (service, _) = fixture();
        create(&service, None).unwrap();
        let paused = service.pause("root").unwrap();
        assert!(service.continuation_prompt(&paused).is_none());
        let resumed = service.resume("root").unwrap();
        assert!(service.continuation_prompt(&resumed).is_some());
    }

    #[test]
    fn the_continuation_prompt_states_the_remaining_budget() {
        let (service, _) = fixture();
        let goal = create(&service, Some(1000)).unwrap();
        let prompt = service.continuation_prompt(&goal).unwrap();
        assert!(prompt.contains("1000 of 1000 tokens left"));
        assert!(prompt.contains("ship the release"));
    }

    #[test]
    fn observing_a_session_with_no_goal_is_not_an_error() {
        let (service, _) = fixture();
        assert!(service.observe("root", &Usage::default()).unwrap().is_none());
    }

    #[test]
    fn elapsed_time_comes_from_the_clock_not_a_guess() {
        let (service, clock) = fixture();
        create(&service, None).unwrap();
        clock.advance_secs(300);
        let goal = service.observe("root", &Usage::default()).unwrap().unwrap();
        assert_eq!(goal.progress.elapsed_secs, 300);
    }
}
