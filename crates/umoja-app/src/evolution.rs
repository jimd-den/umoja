//! Evolution coordinator and stagnation supervisor (NVIDIA AVO implementation).

use std::collections::HashMap;
use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use umoja_domain::error::{DomainError, Result};
use umoja_domain::lineage::{LineageEntry, ParetoFrontier, ScoreVector};
use umoja_domain::ports::LineageStore;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvolutionStatus {
    pub target: String,
    pub total_attempts: u64,
    pub successful_generations: u64,
    pub consecutive_stagnations: u64,
    pub best_score: Option<f64>,
    pub stagnation_detected: bool,
    pub suggested_directions: Vec<String>,
}

pub struct EvolutionCoordinator {
    store: Arc<dyn LineageStore>,
    target: String,
    stagnation_threshold: u64,
}

impl EvolutionCoordinator {
    pub fn new(store: Arc<dyn LineageStore>, target: &str, stagnation_threshold: u64) -> Self {
        Self {
            store,
            target: target.to_string(),
            stagnation_threshold: stagnation_threshold.max(3),
        }
    }

    /// Evaluates candidate result, records progression, and detects stagnation.
    pub fn record_attempt(
        &self,
        attempts: u64,
        rationale: &str,
        scores: ScoreVector,
    ) -> Result<EvolutionStatus> {
        let history = self.store.list(&self.target, 100)?;
        let gen = history.first().map(|e| e.generation + 1).unwrap_or(1);
        let parent_id = history.first().map(|e| e.id.clone());

        let mut frontier = self.store.pareto_frontier(&self.target)?;
        let entry = LineageEntry::new(&format!("lin-{gen:06}"), &self.target, gen, rationale, scores, parent_id)?;

        let is_pareto_improvement = frontier.update(entry.clone());
        if is_pareto_improvement {
            self.store.append(&entry)?;
        }

        let updated_history = self.store.list(&self.target, 100)?;
        let successful_gen = updated_history.len() as u64;
        let consecutive_stagnations = attempts.saturating_sub(successful_gen);
        let stagnation_detected = consecutive_stagnations >= self.stagnation_threshold;

        let mut suggested_directions = Vec::new();
        if stagnation_detected {
            suggested_directions.push("Pivot hypothesis from loop unrolling to register allocation across warp groups".to_string());
            suggested_directions.push("Eliminate warp divergence by switching to speculative branchless execution with relaxed memory fences".to_string());
            suggested_directions.push("Overlap MMA tensor operations with output normalization and correction stages".to_string());
        }

        let best_score = frontier.best().map(|b| b.scores.primary_metric);

        Ok(EvolutionStatus {
            target: self.target.clone(),
            total_attempts: attempts,
            successful_generations: successful_gen,
            consecutive_stagnations,
            best_score,
            stagnation_detected,
            suggested_directions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MemoryLineageStore {
        entries: std::sync::Mutex<Vec<LineageEntry>>,
    }

    impl MemoryLineageStore {
        fn new() -> Self {
            Self { entries: std::sync::Mutex::new(Vec::new()) }
        }
    }

    impl LineageStore for MemoryLineageStore {
        fn append(&self, entry: &LineageEntry) -> Result<()> {
            self.entries.lock().unwrap().push(entry.clone());
            Ok(())
        }

        fn list(&self, _target: &str, limit: usize) -> Result<Vec<LineageEntry>> {
            let mut list = self.entries.lock().unwrap().clone();
            list.reverse();
            list.truncate(limit);
            Ok(list)
        }

        fn pareto_frontier(&self, target: &str) -> Result<ParetoFrontier> {
            let all = self.list(target, 1000)?;
            let mut frontier = ParetoFrontier::new();
            for e in all.into_iter().rev() {
                frontier.update(e);
            }
            Ok(frontier)
        }

        fn get(&self, id: &str) -> Result<Option<LineageEntry>> {
            Ok(self.entries.lock().unwrap().iter().find(|e| e.id == id).cloned())
        }
    }

    #[test]
    fn test_stagnation_detection_and_supervisor_redirection() {
        let store = Arc::new(MemoryLineageStore::new());
        let coord = EvolutionCoordinator::new(store, "attn_kernel.cu", 3);

        // Attempt 1: Success
        let s1 = coord.record_attempt(1, "base kernel", ScoreVector::new("tflops", 1000.0, true)).unwrap();
        assert_eq!(s1.successful_generations, 1);
        assert!(!s1.stagnation_detected);

        // Attempts 2, 3, 4: Failed or non-improving mutations
        let _ = coord.record_attempt(2, "failed attempt 1", ScoreVector::new("tflops", 950.0, true)).unwrap();
        let _ = coord.record_attempt(3, "failed attempt 2", ScoreVector::new("tflops", 980.0, true)).unwrap();
        let s4 = coord.record_attempt(4, "failed attempt 3", ScoreVector::new("tflops", 990.0, true)).unwrap();

        // Stagnation threshold of 3 reached
        assert!(s4.stagnation_detected);
        assert!(!s4.suggested_directions.is_empty());
    }
}
