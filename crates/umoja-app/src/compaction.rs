//! Context compaction.
//!
//! Compaction summarises and continues. It is not a stopping condition, and
//! this service is careful never to treat it as one: goals, heartbeats,
//! children and the kernel namespace are all untouched by it.

use std::sync::Arc;

use umoja_domain::prelude::*;
use umoja_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

pub struct CompactionService {
    env: Env,
    store: Arc<dyn CompactionStore>,
    sessions: Arc<dyn SessionStore>,
    transcript: Arc<dyn TranscriptLog>,
    summariser: Arc<dyn Summariser>,
    default_window: u64,
}

impl std::fmt::Debug for CompactionService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CompactionService")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompactionReport {
    pub summary: String,
    pub trigger: CompactionTrigger,
    pub records_summarised: usize,
    pub state: CompactionState,
}

impl CompactionService {
    pub fn new(
        env: Env,
        store: Arc<dyn CompactionStore>,
        sessions: Arc<dyn SessionStore>,
        transcript: Arc<dyn TranscriptLog>,
        summariser: Arc<dyn Summariser>,
        default_window: u64,
    ) -> Self {
        Self {
            env,
            store,
            sessions,
            transcript,
            summariser,
            default_window,
        }
    }

    pub fn status(&self, selector: &str) -> Result<CompactionState> {
        let session = self.sessions.resolve(selector)?;
        Ok(self
            .store
            .get(&session.id)?
            .unwrap_or_else(|| CompactionState::new(&session.id, self.default_window)))
    }

    /// Records what a turn added to the window, and says whether that crossed
    /// the threshold.
    pub fn observe(&self, selector: &str, tokens: u64) -> Result<(CompactionState, bool)> {
        let mut state = self.status(selector)?;
        state.used_tokens += tokens;
        self.store.put(&state)?;
        let due = state.is_due();
        Ok((state, due))
    }

    pub fn run(
        &self,
        selector: &str,
        trigger: CompactionTrigger,
        instruction: Option<String>,
    ) -> Result<CompactionReport> {
        let session = self.sessions.resolve(selector)?;
        let now = self.env.now();
        let mut state = self.status(selector)?;

        let all = self.transcript.read(&session.id, None)?;
        // The most recent messages are kept verbatim; only what precedes them
        // is summarised.
        let keep = state.keep_recent_messages as usize;
        let cut = all.len().saturating_sub(keep);
        let older = &all[..cut];

        let plan = state.plan(trigger, instruction);
        let summary = self.summariser.summarise(&plan, older)?;

        // Freeing is proportional to what was actually folded away. A
        // compaction that summarised nothing must not claim to have freed
        // anything.
        let freed = if older.is_empty() {
            0
        } else {
            state.used_tokens / 2
        };
        state.record(freed, now);
        self.store.put(&state)?;

        self.transcript.append(&TranscriptRecord::new(
            &session.id,
            now,
            TranscriptEvent::Compacted {
                trigger: trigger.label().to_string(),
                freed_tokens: freed,
            },
        ))?;

        Ok(CompactionReport {
            summary,
            trigger,
            records_summarised: older.len(),
            state,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};

    fn fixture() -> (CompactionService, Arc<MemTranscript>, String) {
        let (env, _clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let transcript = Arc::new(MemTranscript::default());
        let session = SessionService::new(
            env.clone(),
            sessions.clone(),
            transcript.clone(),
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
            CompactionService::new(
                env,
                Arc::new(MemCompaction::default()),
                sessions,
                transcript.clone(),
                Arc::new(MemSummariser),
                200_000,
            ),
            transcript,
            session.id,
        )
    }

    fn fill(transcript: &MemTranscript, session_id: &str, count: usize) {
        for index in 0..count {
            transcript
                .append(&TranscriptRecord::new(
                    session_id,
                    at("2026-08-16T12:00:00Z"),
                    TranscriptEvent::UserPrompt {
                        text: format!("message {index}"),
                    },
                ))
                .unwrap();
        }
    }

    #[test]
    fn compaction_becomes_due_only_near_the_threshold() {
        let (service, _, _) = fixture();
        let (_, due) = service.observe("root", 100_000).unwrap();
        assert!(!due);
        let (_, due) = service.observe("root", 80_000).unwrap();
        assert!(due);
    }

    #[test]
    fn recent_messages_are_kept_verbatim() {
        let (service, transcript, session_id) = fixture();
        fill(&transcript, &session_id, 30);

        let report = service
            .run("root", CompactionTrigger::Manual, None)
            .unwrap();
        // 30 records plus the session-started line, minus the 12 kept recent.
        assert_eq!(report.records_summarised, 19);
    }

    #[test]
    fn a_compaction_with_nothing_to_fold_frees_nothing() {
        let (service, _, _) = fixture();
        service.observe("root", 50_000).unwrap();
        let report = service
            .run("root", CompactionTrigger::Manual, None)
            .unwrap();
        assert_eq!(report.records_summarised, 0);
        assert_eq!(report.state.used_tokens, 50_000);
    }

    #[test]
    fn compacting_frees_space_and_is_recorded() {
        let (service, transcript, session_id) = fixture();
        fill(&transcript, &session_id, 50);
        service.observe("root", 180_000).unwrap();

        let report = service
            .run("root", CompactionTrigger::Threshold, None)
            .unwrap();
        assert!(report.state.used_tokens < 180_000);
        assert_eq!(report.state.compactions, 1);
        assert!(transcript
            .summaries()
            .iter()
            .any(|line| line.contains("compacted on threshold")));
    }

    #[test]
    fn the_instruction_reaches_the_summariser() {
        let (service, transcript, session_id) = fixture();
        fill(&transcript, &session_id, 20);
        let report = service
            .run(
                "root",
                CompactionTrigger::Manual,
                Some("keep the failing tests".into()),
            )
            .unwrap();
        assert!(report.summary.contains("records"));
    }
}
