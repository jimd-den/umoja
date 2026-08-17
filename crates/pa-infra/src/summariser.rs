//! Two ways to summarise a transcript for compaction.
//!
//! [`AgentSummariser`] asks the model, which produces a genuinely useful
//! summary but costs a turn. [`OutlineSummariser`] builds a structured outline
//! from the transcript's own event types, which costs nothing and never
//! hallucinates. The outline is the fallback whenever the runner is unavailable
//! — losing detail is acceptable, inventing it is not.

use std::sync::Arc;

use pa_domain::compaction::CompactionPlan;
use pa_domain::error::Result;
use pa_domain::ports::{AgentRunner, Summariser};
use pa_domain::runner::RunRequest;
use pa_domain::transcript::TranscriptRecord;

pub struct AgentSummariser {
    runner: Arc<dyn AgentRunner>,
    workdir: String,
    model: Option<String>,
}

impl std::fmt::Debug for AgentSummariser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AgentSummariser")
    }
}

impl AgentSummariser {
    pub fn new(runner: Arc<dyn AgentRunner>, workdir: String, model: Option<String>) -> Self {
        Self {
            runner,
            workdir,
            model,
        }
    }
}

impl Summariser for AgentSummariser {
    fn summarise(&self, plan: &CompactionPlan, records: &[TranscriptRecord]) -> Result<String> {
        if records.is_empty() {
            return Ok(String::new());
        }

        let outline = OutlineSummariser.summarise(plan, records)?;
        if self.runner.probe().is_err() {
            return Ok(outline);
        }

        let mut prompt = String::from(
            "Summarise this agent session so work can continue from the summary alone.\n\
             Keep: unfinished work, decisions and their reasons, failing tests, \
             file paths that matter. Drop: pleasantries and superseded detail.\n\
             Do not invent anything that is not below.\n\n",
        );
        if let Some(instruction) = &plan.instruction {
            prompt.push_str(&format!("The user asks specifically: {instruction}\n\n"));
        }
        prompt.push_str(&outline);

        let request = RunRequest::new(&plan.session_id, prompt, &self.workdir)?
            .with_model(self.model.clone());

        match self.runner.run(&request) {
            Ok(outcome) if outcome.ok && !outcome.text.trim().is_empty() => Ok(outcome.text),
            // The model was asked and could not answer. The outline is real
            // information; an error here would throw away the compaction.
            _ => Ok(outline),
        }
    }
}

/// A summary built only from what the transcript already says.
#[derive(Debug, Default)]
pub struct OutlineSummariser;

impl Summariser for OutlineSummariser {
    fn summarise(&self, plan: &CompactionPlan, records: &[TranscriptRecord]) -> Result<String> {
        if records.is_empty() {
            return Ok(String::new());
        }

        let first = records.first().map(|row| row.at);
        let last = records.last().map(|row| row.at);

        let mut out = format!(
            "<summary session=\"{}\" trigger=\"{}\" events=\"{}\">\n",
            plan.session_id,
            plan.trigger.label(),
            records.len()
        );

        if let (Some(first), Some(last)) = (first, last) {
            out.push_str(&format!(
                "Covers {} to {}.\n",
                first.to_rfc3339(),
                last.to_rfc3339()
            ));
        }
        if let Some(instruction) = &plan.instruction {
            out.push_str(&format!("Asked to preserve: {instruction}\n"));
        }

        out.push_str("\nWhat happened, in order:\n");
        for record in records {
            out.push_str(&format!("- {}\n", record.summary()));
        }
        out.push_str("</summary>");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pa_domain::compaction::CompactionTrigger;
    use pa_domain::transcript::TranscriptEvent;

    use crate::runners::DryRunner;

    fn records(count: usize) -> Vec<TranscriptRecord> {
        (0..count)
            .map(|index| {
                TranscriptRecord::new(
                    "ses-1",
                    Utc::now(),
                    TranscriptEvent::UserPrompt {
                        text: format!("prompt {index}"),
                    },
                )
            })
            .collect()
    }

    fn plan() -> CompactionPlan {
        CompactionPlan {
            session_id: "ses-1".into(),
            trigger: CompactionTrigger::Threshold,
            keep_recent_messages: 12,
            instruction: Some("keep the failing tests".into()),
        }
    }

    #[test]
    fn the_outline_lists_events_and_the_instruction() {
        let summary = OutlineSummariser.summarise(&plan(), &records(3)).unwrap();
        assert!(summary.contains("events=\"3\""));
        assert!(summary.contains("keep the failing tests"));
        assert!(summary.contains("prompt 2"));
    }

    #[test]
    fn nothing_to_summarise_produces_nothing() {
        assert!(OutlineSummariser.summarise(&plan(), &[]).unwrap().is_empty());
        let agent = AgentSummariser::new(Arc::new(DryRunner), "/tmp".into(), None);
        assert!(agent.summarise(&plan(), &[]).unwrap().is_empty());
    }

    #[test]
    fn the_agent_summariser_still_produces_a_summary_when_the_model_says_little() {
        // DryRunner echoes the prompt, which is not a real summary — the point
        // of this test is that a useless answer never becomes an error.
        let agent = AgentSummariser::new(Arc::new(DryRunner), "/tmp".into(), None);
        let summary = agent.summarise(&plan(), &records(2)).unwrap();
        assert!(!summary.is_empty());
    }
}
