//! The review half of `/refine`: reading a trajectory back and proposing what
//! is worth keeping.
//!
//! # Why this is a separate step from applying
//!
//! [`crate::harness::HarnessService`] already enforces the rule that matters —
//! nothing is written without evidence. That rule only bites if something
//! stands between *noticing* a lesson and *recording* one. A review that wrote
//! straight to the harness would fill it with confident generalisations drawn
//! from single events, each one carrying a plausible sentence in its evidence
//! field, and the harness would be worse than empty because every entry would
//! look justified.
//!
//! So a review returns proposals. Applying them is a second, deliberate act,
//! and each one applied is an ordinary refinement with an ordinary rollback.

use std::sync::Arc;

use pa_domain::prelude::*;
use pa_domain::transcript::TranscriptRecord;

use crate::harness::{HarnessService, Remember};

/// How much of the trajectory to look back over.
pub const DEFAULT_WINDOW: usize = 40;

#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub proposals: Vec<Proposal>,
    /// How many transcript records the reviewer actually saw.
    pub records_reviewed: usize,
    /// The reviewer's own reply, kept so a surprising result can be read
    /// rather than guessed at.
    pub raw: String,
    /// Set when the reply could not be read as proposals.
    ///
    /// # Why this is not simply an `Err`
    ///
    /// The reply is the only evidence of what went wrong, and returning an
    /// error would throw it away at exactly the moment it is wanted — leaving
    /// `--raw` unable to show the very output that failed to parse. So the
    /// round trip succeeds, the failure is reported, and the text survives.
    pub parse_error: Option<String>,
}

pub struct ReviewService {
    sessions: Arc<dyn SessionStore>,
    transcript: Arc<dyn TranscriptLog>,
    harness: Arc<dyn HarnessStore>,
    runners: Arc<dyn RunnerRegistry>,
    service: Arc<HarnessService>,
}

impl std::fmt::Debug for ReviewService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ReviewService")
    }
}

impl ReviewService {
    pub fn new(
        sessions: Arc<dyn SessionStore>,
        transcript: Arc<dyn TranscriptLog>,
        harness: Arc<dyn HarnessStore>,
        runners: Arc<dyn RunnerRegistry>,
        service: Arc<HarnessService>,
    ) -> Self {
        Self {
            sessions,
            transcript,
            harness,
            runners,
            service,
        }
    }

    /// Reads the recent trajectory and asks the session's own harness what is
    /// worth writing down. Proposes; never writes.
    pub fn review(&self, selector: &str, window: Option<usize>) -> Result<Review> {
        let session = self.sessions.resolve(selector)?;
        let window = window.unwrap_or(DEFAULT_WINDOW).max(1);

        let records = self.transcript.read(&session.id, Some(window))?;
        if records.is_empty() {
            // Nothing happened, so there is nothing to learn from. Asking a
            // model to review an empty trajectory reliably produces invented
            // lessons, which is the exact failure this module exists to avoid.
            return Ok(Review {
                proposals: Vec::new(),
                records_reviewed: 0,
                raw: String::new(),
                parse_error: None,
            });
        }

        let known = self.harness.list(Some(&session.id))?;
        let prompt = review_prompt(&records, &known);

        let run = RunRequest::new(&session.id, prompt, &session.workdir)?
            .with_model(session.model.clone());
        let outcome = self.runners.get(&session.runner)?.run(&run)?;

        if !outcome.ok {
            return Err(DomainError::adapter(
                "review",
                outcome.error.unwrap_or_else(|| "the reviewer failed".into()),
            ));
        }

        let (proposals, parse_error) = match Proposal::parse_many(&outcome.text) {
            Ok(proposals) => (proposals, None),
            Err(error) => (Vec::new(), Some(error.to_string())),
        };

        Ok(Review {
            proposals,
            records_reviewed: records.len(),
            raw: outcome.text,
            parse_error,
        })
    }

    /// Writes accepted proposals as ordinary harness entries.
    ///
    /// Each becomes its own refinement, so a review that got three things
    /// right and one wrong is three commands from being right — rather than
    /// one all-or-nothing decision.
    pub fn apply(&self, selector: &str, proposals: &[Proposal]) -> Result<Vec<Refinement>> {
        let session = self.sessions.resolve(selector)?;
        let mut applied = Vec::new();

        for proposal in proposals {
            let (_, refinement) = self.service.remember(Remember {
                session_id: Some(session.id.clone()),
                name: proposal.name.clone(),
                body: proposal.body.clone(),
                kind: proposal.kind,
                scope: proposal.scope,
                evidence: proposal.evidence.clone(),
                outcome: proposal.outcome.clone(),
                tags: Vec::new(),
            })?;
            applied.push(refinement);
        }

        Ok(applied)
    }
}

/// Builds the reviewer's prompt.
///
/// The four questions are Prime Agent's `/refine` planner's, kept verbatim in
/// spirit because they are what stop a review from being a compliment
/// generator: real evidence, the smallest artifact, a checkable outcome, and
/// the right scope.
fn review_prompt(records: &[TranscriptRecord], known: &[HarnessEntry]) -> String {
    let trajectory = records
        .iter()
        .map(|record| format!("- {}", record.summary()))
        .collect::<Vec<_>>()
        .join("\n");

    let existing = if known.is_empty() {
        "(the harness is empty)".to_string()
    } else {
        known
            .iter()
            .map(|entry| format!("- {}", entry.headline()))
            .collect::<Vec<_>>()
            .join("\n")
    };

    format!(
        "You are reviewing an agent session's own trajectory to decide what, if \
anything, is worth remembering durably.\n\n\
## What already happened\n\n{trajectory}\n\n\
## What the harness already holds\n\n{existing}\n\n\
## Your task\n\n\
Propose harness entries. For each candidate, satisfy all four:\n\n\
1. Is there real evidence? A correction the user had to give, a failure that \
would recur, an explicit preference. One unusual event is not a pattern.\n\
2. What is the smallest artifact? `memory` for a fact, `prompt-note` for a \
behavioural rule, `skill` for a reusable call's contract, `subagent` for a \
delegation role.\n\
3. What should improve, and how would you check? That is the outcome.\n\
4. Local or global? `local` means this project. `global` means a cross-project \
lesson — rare, and worth a second thought.\n\n\
Do not propose anything the harness already holds. Do not propose anything you \
cannot point at a specific moment above to justify. Proposing nothing is a \
good answer and the common one.\n\n\
Reply with a JSON array and nothing else:\n\n\
[{{\"name\": \"kebab-case-slug\", \"kind\": \"memory|prompt-note|skill|subagent\", \
\"scope\": \"local|global\", \"body\": \"the lesson, one or two sentences\", \
\"evidence\": \"the specific moment above that justifies it\", \
\"outcome\": \"what should improve, and how to check\"}}]\n\n\
Reply with [] if nothing qualifies."
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};
    use pa_domain::transcript::TranscriptEvent;

    struct Fixture {
        review: ReviewService,
        sessions_service: SessionService,
        harness_store: Arc<MemHarness>,
        transcript: Arc<MemTranscript>,
        runner: Arc<MemRunner>,
    }

    fn fixture() -> Fixture {
        let (env, _clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let transcript = Arc::new(MemTranscript::default());
        let harness_store = Arc::new(MemHarness::default());
        let runner = Arc::new(MemRunner::ready());
        let runners = Arc::new(MemRunnerRegistry::new(runner.clone()));
        let service = Arc::new(HarnessService::new(
            env.clone(),
            harness_store.clone(),
            transcript.clone(),
        ));

        Fixture {
            review: ReviewService::new(
                sessions.clone(),
                transcript.clone(),
                harness_store.clone(),
                runners,
                service,
            ),
            sessions_service: SessionService::new(
                env,
                sessions,
                transcript.clone(),
                Arc::new(MemSupervisor::default()),
            ),
            harness_store,
            transcript,
            runner,
        }
    }

    fn session(fixture: &Fixture) -> Session {
        fixture
            .sessions_service
            .start(StartSession {
                name: Some("root".into()),
                workdir: "/work".into(),
                runner: "claude".into(),
                model: Some("sonnet".into()),
                kind: SessionKind::Root,
                parent: None,
            })
            .unwrap()
    }

    fn had_a_conversation(fixture: &Fixture, session: &Session) {
        fixture
            .transcript
            .append(&TranscriptRecord::new(
                &session.id,
                at("2026-08-16T12:00:00Z"),
                TranscriptEvent::UserPrompt {
                    text: "use rust, not python".into(),
                },
            ))
            .unwrap();
    }

    fn replies(fixture: &Fixture, text: &str) {
        *fixture.runner.reply.lock().unwrap() = Some(RunOutcome {
            ok: true,
            text: text.into(),
            usage: Usage::default(),
            runner_session: None,
            exit_code: Some(0),
            error: None,
            pid: None,
            duration_ms: 1,
        });
    }

    #[test]
    fn an_empty_trajectory_is_never_sent_to_a_model() {
        let fixture = fixture();
        let session = session(&fixture);
        // A started session has a `SessionStarted` record, so clear the log to
        // get a genuinely empty one.
        fixture.transcript.rows.lock().unwrap().clear();

        let review = fixture.review.review(&session.id, None).unwrap();

        assert!(review.proposals.is_empty());
        assert_eq!(review.records_reviewed, 0);
        // The point: asking a model to review nothing reliably invents
        // lessons, so the round trip is not made at all.
        assert!(fixture.runner.prompts().is_empty());
    }

    #[test]
    fn the_reviewer_is_shown_the_trajectory_and_what_is_already_known() {
        let fixture = fixture();
        let session = session(&fixture);
        had_a_conversation(&fixture, &session);
        replies(&fixture, "[]");

        fixture.review.review(&session.id, None).unwrap();

        let prompt = &fixture.runner.prompts()[0];
        assert!(prompt.contains("use rust, not python"));
        assert!(prompt.contains("the harness is empty"));
        // Proposing nothing must read as a success, not as a broken reviewer.
        assert!(prompt.contains("Proposing nothing is a good answer"));
    }

    #[test]
    fn a_review_proposes_but_never_writes() {
        let fixture = fixture();
        let session = session(&fixture);
        had_a_conversation(&fixture, &session);
        replies(
            &fixture,
            r#"[{"name": "prefers-rust", "body": "Wants memory-safe languages.",
                 "evidence": "Said so explicitly."}]"#,
        );

        let review = fixture.review.review(&session.id, None).unwrap();

        assert_eq!(review.proposals.len(), 1);
        // The entire reason review and apply are two calls.
        assert!(fixture.harness_store.list(Some(&session.id)).unwrap().is_empty());
    }

    #[test]
    fn applying_writes_one_reversible_refinement_per_proposal() {
        let fixture = fixture();
        let session = session(&fixture);
        had_a_conversation(&fixture, &session);
        replies(
            &fixture,
            r#"[{"name": "prefers-rust", "body": "Wants memory-safe languages.",
                 "evidence": "Said so explicitly."},
                {"name": "runs-clippy", "body": "Always run clippy.",
                 "evidence": "Asked for it after the first build."}]"#,
        );

        let review = fixture.review.review(&session.id, None).unwrap();
        let refinements = fixture.review.apply(&session.id, &review.proposals).unwrap();

        // One each, so a review that got one thing wrong is one rollback from
        // being right rather than an all-or-nothing decision.
        assert_eq!(refinements.len(), 2);
        assert_eq!(fixture.harness_store.list(Some(&session.id)).unwrap().len(), 2);
    }

    #[test]
    fn an_unreadable_reply_keeps_the_text_that_could_not_be_read() {
        let fixture = fixture();
        let session = session(&fixture);
        had_a_conversation(&fixture, &session);
        replies(&fixture, r#"here you go: [{"name": }]"#);

        let review = fixture.review.review(&session.id, None).unwrap();

        // Returning an `Err` here would discard the reply at exactly the
        // moment somebody wants to read it.
        assert!(review.proposals.is_empty());
        assert!(review.parse_error.is_some());
        assert!(review.raw.contains("here you go"));
    }

    #[test]
    fn a_reviewer_that_fails_is_reported_rather_than_read_as_nothing_to_learn() {
        let fixture = fixture();
        let session = session(&fixture);
        had_a_conversation(&fixture, &session);
        *fixture.runner.reply.lock().unwrap() =
            Some(RunOutcome::failure("model unavailable", Some(1)));

        // Silently returning zero proposals would be indistinguishable from a
        // clean session, which is how a broken review pass goes unnoticed.
        assert!(fixture.review.review(&session.id, None).is_err());
    }
}
