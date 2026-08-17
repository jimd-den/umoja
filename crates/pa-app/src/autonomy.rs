//! Bounded autonomous mode.
//!
//! The service runs the gates and asks the domain what to do. It never decides
//! on its own, and it never reports "done" when what actually happened was
//! "out of budget" — [`Continuation`] keeps those two outcomes apart all the
//! way to the surface.

use std::sync::Arc;

use pa_domain::prelude::*;
use pa_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

pub struct AutonomyService {
    env: Env,
    store: Arc<dyn AutonomousStore>,
    sessions: Arc<dyn SessionStore>,
    gates: Arc<dyn GateRunner>,
    transcript: Arc<dyn TranscriptLog>,
}

impl std::fmt::Debug for AutonomyService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AutonomyService")
    }
}

/// What one autonomous step concluded, with the evidence behind it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReport {
    pub decision: Continuation,
    pub gates: Vec<GateOutcome>,
    /// Gates that were not re-run because the workspace had not changed since
    /// they last failed.
    pub skipped_gates: Vec<String>,
}

impl AutonomyService {
    pub fn new(
        env: Env,
        store: Arc<dyn AutonomousStore>,
        sessions: Arc<dyn SessionStore>,
        gates: Arc<dyn GateRunner>,
        transcript: Arc<dyn TranscriptLog>,
    ) -> Self {
        Self {
            env,
            store,
            sessions,
            gates,
            transcript,
        }
    }

    pub fn enable(&self, selector: &str, policy: AutonomousPolicy) -> Result<AutonomousState> {
        let session = self.sessions.resolve(selector)?;
        let mut policy = policy;
        policy.enabled = true;
        let state = AutonomousState::new(&session.id, policy, self.env.now());
        self.store.put(&state)?;
        Ok(state)
    }

    pub fn disable(&self, selector: &str) -> Result<()> {
        let session = self.sessions.resolve(selector)?;
        self.store.clear(&session.id)
    }

    pub fn status(&self, selector: &str) -> Result<Option<AutonomousState>> {
        let session = self.sessions.resolve(selector)?;
        self.store.get(&session.id)
    }

    pub fn record_turn(&self, selector: &str, usage: &Usage) -> Result<Option<AutonomousState>> {
        let session = self.sessions.resolve(selector)?;
        let Some(mut state) = self.store.get(&session.id)? else {
            return Ok(None);
        };
        state.record_turn(usage, self.env.now());
        self.store.put(&state)?;
        Ok(Some(state))
    }

    /// Runs the gates that can still tell us something, then decides.
    pub fn step(&self, selector: &str) -> Result<StepReport> {
        let session = self.sessions.resolve(selector)?;
        let now = self.env.now();

        let Some(mut state) = self.store.get(&session.id)? else {
            return Ok(StepReport {
                decision: Continuation::Finish {
                    reason: "autonomous mode is off".into(),
                },
                gates: Vec::new(),
                skipped_gates: Vec::new(),
            });
        };

        let fingerprint = self.gates.fingerprint(&session.workdir).unwrap_or(None);
        let mut ran = Vec::new();
        let mut skipped = Vec::new();

        for gate in state.policy.gates.clone() {
            // A gate that failed against this exact workspace will fail
            // identically. Re-running it costs time and teaches nothing.
            if state.gate_is_stale(&gate.command, fingerprint.as_deref()) {
                skipped.push(gate.command.clone());
                continue;
            }

            let outcome = self.gates.run(&gate, &session.workdir)?;
            self.transcript.append(&TranscriptRecord::new(
                &session.id,
                now,
                TranscriptEvent::GateRan {
                    command: outcome.command.clone(),
                    passed: outcome.passed,
                    exit_code: outcome.exit_code,
                },
            ))?;
            state.record_gate(outcome.clone());
            ran.push(outcome);
        }

        let decision = state.decide(now);
        if decision.should_continue() {
            state.record_continuation(now);
        }
        self.store.put(&state)?;

        self.transcript.append(&TranscriptRecord::new(
            &session.id,
            now,
            TranscriptEvent::AutonomousDecision {
                decision: decision.label().to_string(),
                reason: decision.reason().to_string(),
            },
        ))?;

        Ok(StepReport {
            decision,
            gates: ran,
            skipped_gates: skipped,
        })
    }

    /// The follow-up text handed back to the agent when a gate failed.
    ///
    /// It carries the gate's own output verbatim (already clipped by the
    /// adapter). Paraphrasing a compiler error helps nobody.
    pub fn continuation_prompt(&self, report: &StepReport) -> Option<String> {
        if !report.decision.should_continue() {
            return None;
        }
        let failing: Vec<&GateOutcome> = report.gates.iter().filter(|g| !g.passed).collect();
        if failing.is_empty() {
            return Some(format!("<autonomous>{}</autonomous>", report.decision.reason()));
        }

        let mut prompt = String::from("<autonomous>\nA quality gate is failing.\n");
        for gate in failing {
            prompt.push_str(&format!(
                "\n$ {}\nexit {}\n{}\n",
                gate.command,
                gate.exit_code.unwrap_or(-1),
                gate.output.trim()
            ));
        }
        prompt.push_str("\nFix the cause, then let the gate run again.\n</autonomous>");
        Some(prompt)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};

    fn fixture() -> (AutonomyService, Arc<MemGates>) {
        let (env, _clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let gates = Arc::new(MemGates::default());
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
            AutonomyService::new(
                env,
                Arc::new(MemAutonomous::default()),
                sessions,
                gates.clone(),
                Arc::new(MemTranscript::default()),
            ),
            gates,
        )
    }

    fn policy(commands: &[&str]) -> AutonomousPolicy {
        AutonomousPolicy {
            enabled: true,
            gates: commands
                .iter()
                .map(|command| Gate::new(*command).unwrap())
                .collect(),
            limits: AutonomousLimits::default(),
        }
    }

    #[test]
    fn a_failing_gate_produces_a_continuation_carrying_its_output() {
        let (service, gates) = fixture();
        service.enable("root", policy(&["npm run check"])).unwrap();
        gates
            .results
            .lock()
            .unwrap()
            .push(("npm run check".into(), false));

        let report = service.step("root").unwrap();
        assert!(report.decision.should_continue());
        let prompt = service.continuation_prompt(&report).unwrap();
        assert!(prompt.contains("npm run check"));
        assert!(prompt.contains("boom"));
    }

    #[test]
    fn passing_gates_finish_rather_than_continue() {
        let (service, _) = fixture();
        service.enable("root", policy(&["npm run check"])).unwrap();
        let report = service.step("root").unwrap();
        assert_eq!(report.decision.label(), "finish");
        assert!(service.continuation_prompt(&report).is_none());
    }

    #[test]
    fn an_unchanged_workspace_does_not_rerun_a_failed_gate() {
        let (service, gates) = fixture();
        service.enable("root", policy(&["npm run check"])).unwrap();
        *gates.fingerprint.lock().unwrap() = Some("abc123".into());
        gates
            .results
            .lock()
            .unwrap()
            .push(("npm run check".into(), false));

        service.step("root").unwrap();
        let second = service.step("root").unwrap();

        assert_eq!(gates.runs.lock().unwrap().len(), 1);
        assert_eq!(second.skipped_gates, vec!["npm run check".to_string()]);
        assert!(second.decision.should_continue());
    }

    #[test]
    fn a_changed_workspace_reruns_the_gate() {
        let (service, gates) = fixture();
        service.enable("root", policy(&["npm run check"])).unwrap();
        *gates.fingerprint.lock().unwrap() = Some("abc123".into());
        gates
            .results
            .lock()
            .unwrap()
            .push(("npm run check".into(), false));

        service.step("root").unwrap();
        *gates.fingerprint.lock().unwrap() = Some("def456".into());
        service.step("root").unwrap();

        assert_eq!(gates.runs.lock().unwrap().len(), 2);
    }

    #[test]
    fn hitting_a_limit_stops_and_says_so() {
        let (service, _) = fixture();
        let mut policy = policy(&[]);
        policy.limits.max_continuations = 1;
        service.enable("root", policy).unwrap();

        // Nothing to gate, so the first step finishes; force a continuation
        // count instead and confirm the label is "stop", never "finish".
        let mut state = service.status("root").unwrap().unwrap();
        state.continuations = 1;
        service.store.put(&state).unwrap();

        let report = service.step("root").unwrap();
        assert_eq!(report.decision.label(), "stop");
        assert!(report.decision.reason().contains("continuation limit"));
    }

    #[test]
    fn autonomy_off_is_not_an_error() {
        let (service, _) = fixture();
        let report = service.step("root").unwrap();
        assert_eq!(report.decision.label(), "finish");
        assert!(report.gates.is_empty());
    }

    #[test]
    fn turns_accumulate_towards_the_token_limit() {
        let (service, _) = fixture();
        service.enable("root", policy(&[])).unwrap();
        let usage = Usage {
            input_tokens: 1000,
            output_tokens: 500,
            turns: 1,
            attributed_child_tokens: 0,
        };
        service.record_turn("root", &usage).unwrap();
        let state = service.record_turn("root", &usage).unwrap().unwrap();
        assert_eq!(state.usage.total_tokens(), 3000);
        assert_eq!(state.turns, 2);
    }
}
