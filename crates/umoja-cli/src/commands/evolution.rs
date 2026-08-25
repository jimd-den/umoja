//! Evolution CLI commands (NVIDIA AVO implementation).

use umoja_domain::error::Result;

use crate::cli::EvolveCommand;
use crate::output::Output;
use crate::wiring::App;

pub fn evolve(app: &App, cmd: &EvolveCommand) -> Result<Output> {
    match cmd {
        EvolveCommand::Lineage { target, limit } => {
            let entries = app.lineage.list(target, *limit)?;
            let json_val = serde_json::to_value(&entries).unwrap_or_default();
            let mut lines = vec![format!("Lineage history for target: {target} (total: {})", entries.len())];
            for e in &entries {
                let score_str = format!("{}: {:.2}", e.scores.primary_metric_name, e.scores.primary_metric);
                let commit_str = e.commit_hash.as_deref().unwrap_or("-");
                let tree_indent = "  ".repeat(e.depth.saturating_sub(1) as usize);
                let scale_str = format!("({:.1}x scale)", e.scale_multiplier);
                lines.push(format!("  {}└─ Gen {:<2} [D:{:<2}] [{:<7}] -> {:<15} {:<12} | {}", tree_indent, e.generation, e.depth, commit_str, score_str, scale_str, e.rationale));
            }
            Ok(Output::new(lines.join("\n"), json_val))
        }
        EvolveCommand::Best { target } => {
            let frontier = app.lineage.pareto_frontier(target)?;
            if let Some(best) = frontier.best() {
                let json_val = serde_json::to_value(best).unwrap_or_default();
                let text = format!(
                    "Pareto Optimal Solution for {target}:\n  ID:         {}\n  Generation: {}\n  Score:      {}: {:.2}\n  Commit:     {}\n  Rationale:  {}",
                    best.id,
                    best.generation,
                    best.scores.primary_metric_name,
                    best.scores.primary_metric,
                    best.commit_hash.as_deref().unwrap_or("-"),
                    best.rationale
                );
                Ok(Output::new(text, json_val))
            } else {
                Ok(Output::message(format!("No recorded solutions found for {target}.")))
            }
        }
        EvolveCommand::Status { target } => {
            let history = app.lineage.list(target, 100)?;
            let frontier = app.lineage.pareto_frontier(target)?;
            let best_score = frontier.best().map(|b| b.scores.primary_metric).unwrap_or(0.0);
            let json_val = serde_json::json!({
                "target": target,
                "generations": history.len(),
                "best_score": best_score,
            });
            let text = format!(
                "Evolution Status for {target}:\n  Committed Generations: {}\n  Best Primary Score:    {:.2}",
                history.len(),
                best_score
            );
            Ok(Output::new(text, json_val))
        }
    }
}

