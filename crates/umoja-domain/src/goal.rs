use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};
use crate::session::Usage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    Active,
    Paused,
    Completed,
    /// Stopped because a configured budget ran out, not because it was done.
    BudgetExhausted,
    Errored,
    Cleared,
}

impl GoalStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::BudgetExhausted | Self::Errored | Self::Cleared
        )
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Completed => "completed",
            Self::BudgetExhausted => "budget-exhausted",
            Self::Errored => "errored",
            Self::Cleared => "cleared",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalBudget {
    pub tokens: Option<u64>,
    pub wall_clock_secs: Option<u64>,
    pub continuations: Option<u32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalProgress {
    pub tokens_used: u64,
    pub continuations: u32,
    pub elapsed_secs: u64,
}

/// A discrete step or task item within an overarching goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    pub id: u32,
    pub text: String,
    pub completed: bool,
    pub completed_at: Option<DateTime<Utc>>,
}

impl ChecklistItem {
    pub fn new(id: u32, text: impl Into<String>) -> Self {
        Self {
            id,
            text: text.into().trim().to_string(),
            completed: false,
            completed_at: None,
        }
    }
}

/// A durable objective that outlives a turn.
///
/// The rule that matters is at the bottom of this file: only [`Goal::complete`]
/// marks success. Running out of budget, being paused and being cleared are all
/// distinct outcomes, because a harness that reports "done" when it actually
/// ran out of money is worse than one that reports nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Goal {
    pub session_id: String,
    pub objective: String,
    pub status: GoalStatus,
    #[serde(default)]
    pub budget: GoalBudget,
    #[serde(default)]
    pub progress: GoalProgress,
    #[serde(default)]
    pub checklist: Vec<ChecklistItem>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    /// Why it stopped, when it stopped for a reason worth reading.
    pub note: Option<String>,
}

impl Goal {
    pub fn new(
        session_id: impl Into<String>,
        objective: impl Into<String>,
        budget: GoalBudget,
        now: DateTime<Utc>,
    ) -> Result<Self> {
        let objective = objective.into().trim().to_string();
        if objective.is_empty() {
            return Err(DomainError::invalid("a goal needs an objective"));
        }
        Ok(Self {
            session_id: session_id.into(),
            objective,
            status: GoalStatus::Active,
            budget,
            progress: GoalProgress::default(),
            checklist: Vec::new(),
            created_at: now,
            updated_at: now,
            completed_at: None,
            note: None,
        })
    }

    /// Records what a turn cost and re-checks the budget.
    ///
    /// Returns true when this observation exhausted the budget, so the caller
    /// can say so rather than discovering it on the next continuation.
    pub fn observe(&mut self, usage: &Usage, now: DateTime<Utc>) -> bool {
        self.progress.tokens_used += usage.total_tokens();
        self.progress.elapsed_secs = (now - self.created_at).num_seconds().max(0) as u64;
        self.updated_at = now;

        if self.status == GoalStatus::Active && self.over_budget() {
            self.status = GoalStatus::BudgetExhausted;
            self.note = Some("stopped on budget, not on completion".to_string());
            return true;
        }
        false
    }

    pub fn over_budget(&self) -> bool {
        let GoalBudget {
            tokens,
            wall_clock_secs,
            continuations,
        } = self.budget;
        tokens.is_some_and(|limit| self.progress.tokens_used >= limit)
            || wall_clock_secs.is_some_and(|limit| self.progress.elapsed_secs >= limit)
            || continuations.is_some_and(|limit| self.progress.continuations >= limit)
    }

    /// Should the harness present this goal again after an ordinary turn?
    pub fn should_continue(&self) -> bool {
        self.status == GoalStatus::Active && !self.over_budget()
    }

    pub fn record_continuation(&mut self, now: DateTime<Utc>) {
        self.progress.continuations += 1;
        self.updated_at = now;
    }

    pub fn pause(&mut self, now: DateTime<Utc>) -> Result<()> {
        self.transition_from_live(now, GoalStatus::Paused, "pause")
    }

    pub fn resume(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status != GoalStatus::Paused {
            return Err(DomainError::forbidden(format!(
                "only a paused goal can resume; this one is {}",
                self.status.label()
            )));
        }
        self.status = GoalStatus::Active;
        self.updated_at = now;
        Ok(())
    }

    /// The only way to succeed.
    pub fn complete(&mut self, now: DateTime<Utc>) -> Result<()> {
        if self.status.is_terminal() {
            return Err(DomainError::forbidden(format!(
                "this goal already ended as {}",
                self.status.label()
            )));
        }
        self.status = GoalStatus::Completed;
        self.completed_at = Some(now);
        self.updated_at = now;
        Ok(())
    }

    pub fn clear(&mut self, now: DateTime<Utc>) {
        self.status = GoalStatus::Cleared;
        self.updated_at = now;
    }

    pub fn fail(&mut self, reason: impl Into<String>, now: DateTime<Utc>) {
        self.status = GoalStatus::Errored;
        self.note = Some(reason.into());
        self.updated_at = now;
    }

    pub fn remaining_tokens(&self) -> Option<u64> {
        self.budget
            .tokens
            .map(|limit| limit.saturating_sub(self.progress.tokens_used))
    }

    pub fn elapsed(&self) -> Duration {
        Duration::seconds(self.progress.elapsed_secs as i64)
    }

    // -------------------------------------------------------------------------
    // Checklist Operations
    // -------------------------------------------------------------------------

    pub fn add_step(&mut self, text: impl Into<String>, now: DateTime<Utc>) -> Result<u32> {
        let text = text.into().trim().to_string();
        if text.is_empty() {
            return Err(DomainError::invalid("checklist step cannot be empty"));
        }
        let next_id = self.checklist.iter().map(|item| item.id).max().unwrap_or(0) + 1;
        self.checklist.push(ChecklistItem::new(next_id, text));
        self.updated_at = now;
        Ok(next_id)
    }
    pub fn check_step(&mut self, id: u32, now: DateTime<Utc>) -> Result<bool> {
        if let Some(item) = self.checklist.iter_mut().find(|i| i.id == id) {
            if !item.completed {
                item.completed = true;
                item.completed_at = Some(now);
                self.updated_at = now;
                return Ok(true);
            }
            return Ok(false);
        }
        Err(DomainError::not_found("checklist_item", id.to_string()))
    }

    pub fn uncheck_step(&mut self, id: u32, now: DateTime<Utc>) -> Result<bool> {
        if let Some(item) = self.checklist.iter_mut().find(|i| i.id == id) {
            if item.completed {
                item.completed = false;
                item.completed_at = None;
                self.updated_at = now;
                return Ok(true);
            }
            return Ok(false);
        }
        Err(DomainError::not_found("checklist_item", id.to_string()))
    }

    pub fn remove_step(&mut self, id: u32, now: DateTime<Utc>) -> Result<bool> {
        let initial_len = self.checklist.len();
        self.checklist.retain(|i| i.id != id);
        if self.checklist.len() != initial_len {
            self.updated_at = now;
            return Ok(true);
        }
        Err(DomainError::not_found("checklist_item", id.to_string()))
    }

    pub fn next_step(&self) -> Option<&ChecklistItem> {
        self.checklist.iter().find(|i| !i.completed)
    }

    pub fn all_steps_completed(&self) -> bool {
        !self.checklist.is_empty() && self.checklist.iter().all(|i| i.completed)
    }

    pub fn progress_summary(&self) -> String {
        if self.checklist.is_empty() {
            return "no steps defined".to_string();
        }
        let done = self.checklist.iter().filter(|i| i.completed).count();
        let total = self.checklist.len();
        let pct = (done * 100) / total;
        format!("{done}/{total} steps complete ({pct}%)")
    }

    fn transition_from_live(
        &mut self,
        now: DateTime<Utc>,
        to: GoalStatus,
        verb: &str,
    ) -> Result<()> {
        if self.status.is_terminal() {
            return Err(DomainError::forbidden(format!(
                "cannot {verb} a goal that already ended as {}",
                self.status.label()
            )));
        }
        self.status = to;
        self.updated_at = now;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-08-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn goal_with(budget: GoalBudget) -> Goal {
        Goal::new("ses-1", "ship the release", budget, now()).unwrap()
    }

    #[test]
    fn a_goal_needs_an_objective() {
        assert!(Goal::new("ses-1", "   ", GoalBudget::default(), now()).is_err());
    }

    #[test]
    fn budget_exhaustion_is_not_completion() {
        let mut goal = goal_with(GoalBudget {
            tokens: Some(100),
            ..Default::default()
        });
        let exhausted = goal.observe(
            &Usage {
                input_tokens: 80,
                output_tokens: 40,
                ..Default::default()
            },
            now(),
        );
        assert!(exhausted);
        assert_eq!(goal.status, GoalStatus::BudgetExhausted);
        assert!(!goal.should_continue());
        // And it cannot be quietly relabelled as success afterwards.
        assert!(goal.complete(now()).is_err());
    }

    #[test]
    fn only_complete_marks_success() {
        let mut goal = goal_with(GoalBudget::default());
        goal.pause(now()).unwrap();
        assert!(!goal.should_continue());
        goal.resume(now()).unwrap();
        assert!(goal.should_continue());
        goal.complete(now()).unwrap();
        assert_eq!(goal.status, GoalStatus::Completed);
        assert!(goal.completed_at.is_some());
    }

    #[test]
    fn a_cleared_goal_cannot_resume() {
        let mut goal = goal_with(GoalBudget::default());
        goal.clear(now());
        assert!(goal.resume(now()).is_err());
        assert!(goal.pause(now()).is_err());
    }

    #[test]
    fn checklist_lifecycle_add_check_uncheck_progress() {
        let mut goal = goal_with(GoalBudget::default());
        let id1 = goal.add_step("Write domain model", now()).unwrap();
        let id2 = goal.add_step("Add application service", now()).unwrap();
        assert_eq!(id1, 1);
        assert_eq!(id2, 2);

        assert_eq!(goal.progress_summary(), "0/2 steps complete (0%)");
        assert_eq!(goal.next_step().unwrap().text, "Write domain model");

        // Check first step
        let checked = goal.check_step(id1, now()).unwrap();
        assert!(checked);
        assert_eq!(goal.progress_summary(), "1/2 steps complete (50%)");
        assert_eq!(goal.next_step().unwrap().text, "Add application service");

        // Check second step
        goal.check_step(id2, now()).unwrap();
        assert!(goal.all_steps_completed());
        assert_eq!(goal.progress_summary(), "2/2 steps complete (100%)");

        // Uncheck
        goal.uncheck_step(id1, now()).unwrap();
        assert_eq!(goal.progress_summary(), "1/2 steps complete (50%)");

        // Remove
        goal.remove_step(id1, now()).unwrap();
        assert_eq!(goal.progress_summary(), "1/1 steps complete (100%)");
    }
}
