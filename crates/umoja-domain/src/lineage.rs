//! Lineage and Evolutionary Optimization domain models (inspired by NVIDIA AVO).

use std::collections::HashMap;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{DomainError, Result};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreVector {
    pub correctness: bool,
    pub primary_metric: f64,
    pub primary_metric_name: String,
    #[serde(default)]
    pub metrics: HashMap<String, f64>,
}

impl ScoreVector {
    pub fn new(metric_name: &str, value: f64, correct: bool) -> Self {
        let mut metrics = HashMap::new();
        metrics.insert(metric_name.to_string(), value);
        Self {
            correctness: correct,
            primary_metric: value,
            primary_metric_name: metric_name.to_string(),
            metrics,
        }
    }

    /// Returns true if `self` Pareto-dominates `other` (higher is better for metrics, must be correct).
    pub fn dominates(&self, other: &ScoreVector) -> bool {
        if !self.correctness {
            return false;
        }
        if !other.correctness {
            return true;
        }

        let mut strictly_better = false;
        if self.primary_metric < other.primary_metric {
            return false;
        } else if self.primary_metric > other.primary_metric {
            strictly_better = true;
        }

        for (k, v) in &other.metrics {
            if let Some(self_v) = self.metrics.get(k) {
                if *self_v < *v {
                    return false;
                } else if *self_v > *v {
                    strictly_better = true;
                }
            }
        }

        strictly_better
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LineageEntry {
    pub id: String,
    pub target: String,
    pub generation: u64,
    pub depth: u64,
    pub scale_multiplier: f64,
    pub commit_hash: Option<String>,
    pub parent_id: Option<String>,
    #[serde(default)]
    pub ancestor_ids: Vec<String>,
    pub rationale: String,
    pub scores: ScoreVector,
    pub hardware_profile: Option<HashMap<String, String>>,
    pub created_at: DateTime<Utc>,
}

impl LineageEntry {
    pub fn new(
        id: &str,
        target: &str,
        generation: u64,
        rationale: &str,
        scores: ScoreVector,
        parent_id: Option<String>,
    ) -> Result<Self> {
        if rationale.trim().is_empty() {
            return Err(DomainError::invalid("Lineage entry requires a non-empty rationale"));
        }
        Ok(Self {
            id: id.to_string(),
            target: target.to_string(),
            generation,
            depth: generation,
            scale_multiplier: 1.0,
            commit_hash: None,
            parent_id,
            ancestor_ids: Vec::new(),
            rationale: rationale.to_string(),
            scores,
            hardware_profile: None,
            created_at: Utc::now(),
        })
    }

    /// Recursively computes the evolutionary depth and ancestral chain.
    pub fn with_ancestry(mut self, depth: u64, ancestors: Vec<String>, scale_factor: f64) -> Self {
        self.depth = depth;
        self.ancestor_ids = ancestors;
        self.scale_multiplier = scale_factor;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParetoFrontier {
    pub entries: Vec<LineageEntry>,
}

impl ParetoFrontier {
    pub fn new() -> Self {
        Self { entries: Vec::new() }
    }

    /// Evaluates candidate against frontier; updates frontier if non-dominated and returns true.
    pub fn update(&mut self, candidate: LineageEntry) -> bool {
        if !candidate.scores.correctness {
            return false;
        }

        // If any existing entry dominates candidate, reject
        if self.entries.iter().any(|e| e.scores.dominates(&candidate.scores)) {
            return false;
        }

        // Remove entries dominated by new candidate
        self.entries.retain(|e| !candidate.scores.dominates(&e.scores));
        self.entries.push(candidate);
        true
    }

    pub fn best(&self) -> Option<&LineageEntry> {
        self.entries.iter().max_by(|a, b| {
            a.scores
                .primary_metric
                .partial_cmp(&b.scores.primary_metric)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_score_vector_domination() {
        let v1 = ScoreVector::new("tflops", 1500.0, true);
        let v2 = ScoreVector::new("tflops", 1400.0, true);
        let v_bad = ScoreVector::new("tflops", 1600.0, false);

        assert!(v1.dominates(&v2));
        assert!(!v2.dominates(&v1));
        assert!(v1.dominates(&v_bad));
    }

    #[test]
    fn test_pareto_frontier_updates() {
        let mut frontier = ParetoFrontier::new();
        let e1 = LineageEntry::new("lin-001", "kernel.cu", 1, "initial", ScoreVector::new("tflops", 1000.0, true), None).unwrap();
        let e2 = LineageEntry::new("lin-002", "kernel.cu", 2, "better", ScoreVector::new("tflops", 1200.0, true), Some(e1.id.clone())).unwrap();
        let e_bad = LineageEntry::new("lin-003", "kernel.cu", 3, "failed", ScoreVector::new("tflops", 1400.0, false), None).unwrap();

        assert!(frontier.update(e1));
        assert_eq!(frontier.best().unwrap().scores.primary_metric, 1000.0);

        assert!(frontier.update(e2));
        assert_eq!(frontier.best().unwrap().scores.primary_metric, 1200.0);
        assert_eq!(frontier.entries.len(), 1); // e1 was dominated

        assert!(!frontier.update(e_bad));
    }
}
