//! Recursive delegation.
//!
//! [`SubagentService::spawn`] returns the instant a child is *admitted*. It does
//! not wait, and it does not carry an answer. That is not a limitation to work
//! around — it is the feature. A parent that blocked on its children could only
//! ever fan out one level and would spend the whole run idle; a parent that
//! ends its turn after admitting three children can be doing something else
//! while all three work, and hears back through messages or files.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use pa_domain::prelude::*;
use pa_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

#[derive(Debug, Clone)]
pub struct Spawn {
    pub parent_selector: String,
    pub prompt: String,
    pub name: Option<String>,
    pub model: Option<String>,
    /// Defaults to the parent's runner: a panel that all thinks alike is not a
    /// panel, but a child that cannot be resumed is worse.
    pub runner: Option<String>,
    pub system_prompt: Option<String>,
}

pub struct SubagentService {
    env: Env,
    sessions: Arc<dyn SessionStore>,
    registry: Arc<dyn SubagentRegistry>,
    runners: Arc<dyn RunnerRegistry>,
    transcript: Arc<dyn TranscriptLog>,
    depth: DepthPolicy,
}

impl std::fmt::Debug for SubagentService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SubagentService")
    }
}

impl SubagentService {
    pub fn new(
        env: Env,
        sessions: Arc<dyn SessionStore>,
        registry: Arc<dyn SubagentRegistry>,
        runners: Arc<dyn RunnerRegistry>,
        transcript: Arc<dyn TranscriptLog>,
        depth: DepthPolicy,
    ) -> Self {
        Self {
            env,
            sessions,
            registry,
            runners,
            transcript,
            depth,
        }
    }

    /// Admits a child: creates its session, registers it, and hands back
    /// everything a run needs.
    ///
    /// Shared by [`Self::spawn`] and [`Self::call`], which differ only in
    /// whether they wait for the answer.
    fn admit(&self, request: &Spawn, now: DateTime<Utc>) -> Result<(Subagent, Session)> {
        let parent = self.sessions.resolve(&request.parent_selector)?;

        // Depth is checked before anything is created, so a refusal leaves no
        // orphan session behind.
        let child_depth = self.depth.admit(parent.depth)?;

        let prompt = request.prompt.trim();
        if prompt.is_empty() {
            return Err(DomainError::invalid("a subagent needs a prompt"));
        }

        let child_id = self.env.id(Ids::SUBAGENT);
        let session_id = self.env.id(Ids::SESSION);
        let name = match request.name.clone() {
            Some(raw) => Session::normalise_name(&raw)?,
            None => Session::normalise_name(&child_id)?,
        };

        if self.registry.get(&parent.id, &name).is_ok() {
            return Err(DomainError::conflict("subagent name", name));
        }

        let runner_name = request
            .runner
            .clone()
            .unwrap_or_else(|| parent.runner.clone());
        // An exact model was asked for or the parent's is inherited. There is
        // deliberately no fallback to "some other model that is available":
        // silently answering with a different mind than the one requested is
        // worse than failing.
        let model = request
            .model
            .clone()
            .or_else(|| parent.model.clone())
            .unwrap_or_else(|| "default".to_string());

        let session_dir = format!("{}/{}", parent.workdir, child_id);

        let child_session = Session {
            id: session_id.clone(),
            name: name.clone(),
            kind: SessionKind::Child,
            status: SessionStatus::Running,
            workdir: parent.workdir.clone(),
            runner: runner_name.clone(),
            model: Some(model.clone()),
            parent_id: Some(parent.id.clone()),
            depth: child_depth,
            created_at: now,
            updated_at: now,
            usage: Usage::default(),
            pid: None,
        };
        self.sessions.insert(&child_session)?;

        let child = Subagent {
            child_id,
            parent_session_id: parent.id.clone(),
            session_id,
            name,
            session_dir,
            model,
            runner: runner_name,
            prompt: prompt.to_string(),
            depth: child_depth,
            status: SubagentStatus::Admitted,
            created_at: now,
            updated_at: now,
            usage: Usage::default(),
            usage_attributed: false,
            last_error: None,
        };
        self.registry.insert(&child)?;

        Ok((child, child_session))
    }

    /// Records a child that never got off the ground.
    ///
    /// It stays in the registry as a failed entry rather than vanishing,
    /// because a delegation that never ran is something the parent needs to
    /// find out about.
    fn fail_to_launch(
        &self,
        child: &mut Subagent,
        now: DateTime<Utc>,
        error: &DomainError,
    ) -> Result<()> {
        child.status = SubagentStatus::Failed;
        child.last_error = Some(error.to_string());
        self.registry.update(child)?;
        self.transcript.append(&TranscriptRecord::new(
            &child.parent_session_id,
            now,
            TranscriptEvent::Error {
                context: format!("spawn {}", child.name),
                detail: error.to_string(),
            },
        ))
    }

    fn note_admitted(&self, child: &Subagent, now: DateTime<Utc>) -> Result<()> {
        self.transcript.append(&TranscriptRecord::new(
            &child.parent_session_id,
            now,
            TranscriptEvent::SubagentAdmitted {
                child_id: child.child_id.clone(),
                name: child.name.clone(),
                model: child.model.clone(),
            },
        ))
    }

    /// Admits a child and returns its handle. Never its answer.
    pub fn spawn(&self, request: Spawn) -> Result<SpawnHandle> {
        let now = self.env.now();
        let (mut child, child_session) = self.admit(&request, now)?;

        let run = RunRequest::new(&child.session_id, &child.prompt, &child_session.workdir)?
            .with_model(Some(child.model.clone()))
            .with_system_prompt(request.system_prompt)
            .detached();

        // `--with opencode` on a spawn means the child really runs there, even
        // though the parent is on something else.
        match self
            .runners
            .get(&child.runner)
            .and_then(|runner| runner.run(&run))
        {
            Ok(outcome) => {
                child.status = SubagentStatus::Running;
                if let Some(pid) = outcome.pid {
                    let mut session = child_session;
                    session.pid = Some(pid);
                    self.sessions.update(&session)?;
                }
            }
            Err(error) => {
                self.fail_to_launch(&mut child, now, &error)?;
                return Err(error);
            }
        }

        self.registry.update(&child)?;
        self.note_admitted(&child, now)?;

        Ok(child.handle())
    }

    /// Delegates and **waits** — `rlm(...)` used as a function call rather
    /// than a fan-out.
    ///
    /// # Why this exists alongside [`Self::spawn`]
    ///
    /// Fan-out is the right shape whenever the parent has other work to do,
    /// and it is why `spawn` refuses to block. But a child's answer is
    /// sometimes the *input to the next line of code* — a second opinion to
    /// compare against, a classification to branch on, a summary to index.
    /// Routing that through an inbox means writing the continuation as a
    /// separate turn, which is a great deal of ceremony for one value.
    ///
    /// So this waits, and pays for waiting: one child at a time, the parent
    /// idle meanwhile. Reach for [`Self::spawn`] whenever the answer is not
    /// needed before the turn ends.
    pub fn call(&self, request: Spawn, timeout_secs: Option<u64>) -> Result<CallResult> {
        let now = self.env.now();
        let (mut child, child_session) = self.admit(&request, now)?;

        let mut run = RunRequest::new(&child.session_id, &child.prompt, &child_session.workdir)?
            .with_model(Some(child.model.clone()))
            .with_system_prompt(request.system_prompt);
        if let Some(seconds) = timeout_secs {
            run.timeout_secs = seconds;
        }

        // Marked running before the call rather than after it: this blocks for
        // as long as the child takes, and anything inspecting the registry
        // meanwhile should see work in progress, not a child stuck at
        // `admitted`.
        child.status = SubagentStatus::Running;
        self.registry.update(&child)?;
        self.note_admitted(&child, now)?;

        let outcome = match self
            .runners
            .get(&child.runner)
            .and_then(|runner| runner.run(&run))
        {
            Ok(outcome) => outcome,
            Err(error) => {
                self.fail_to_launch(&mut child, now, &error)?;
                return Err(error);
            }
        };

        // A child that ran and failed is still a child that ran, so it is
        // settled with what it actually cost. The caller is told it failed
        // rather than handed an empty string that reads like an answer.
        let status = if outcome.ok {
            SubagentStatus::Completed
        } else {
            child.last_error = outcome.error.clone();
            self.registry.update(&child)?;
            SubagentStatus::Failed
        };
        let settled = self.settle(&request.parent_selector, &child.name, status, outcome.usage)?;

        Ok(CallResult {
            handle: settled.handle(),
            ok: outcome.ok,
            text: outcome.text,
            usage: outcome.usage,
            error: outcome.error,
            duration_ms: outcome.duration_ms,
        })
    }

    pub fn list(&self, parent_selector: &str, include_deleted: bool) -> Result<Vec<Subagent>> {
        let parent = self.sessions.resolve(parent_selector)?;
        let mut rows = self.registry.list(&parent.id, include_deleted)?;
        rows.sort_by_key(|row| row.created_at);
        Ok(rows)
    }

    pub fn get(&self, parent_selector: &str, selector: &str) -> Result<Subagent> {
        let parent = self.sessions.resolve(parent_selector)?;
        self.registry.get(&parent.id, selector)
    }

    /// Records a child finishing, and folds its cost into the parent.
    ///
    /// Attribution happens exactly once — the `usage_attributed` flag exists so
    /// that replaying a registry after a restart cannot double-charge a parent
    /// for the same child.
    pub fn settle(
        &self,
        parent_selector: &str,
        selector: &str,
        status: SubagentStatus,
        usage: Usage,
    ) -> Result<Subagent> {
        let now = self.env.now();
        let parent = self.sessions.resolve(parent_selector)?;
        let mut child = self.registry.get(&parent.id, selector)?;

        child.usage = usage;
        child.settle(status, now);

        if !child.usage_attributed {
            let mut parent_session = self.sessions.get(&parent.id)?;
            parent_session.usage.attribute_child(&child.usage);
            parent_session.updated_at = now;
            self.sessions.update(&parent_session)?;
            child.usage_attributed = true;

            self.transcript.append(&TranscriptRecord::new(
                &parent.id,
                now,
                TranscriptEvent::ChildUsageAttributed {
                    child_id: child.child_id.clone(),
                    child_usage: child.usage,
                    aggregate: parent_session.usage,
                },
            ))?;
        }

        self.registry.update(&child)?;
        self.transcript.append(&TranscriptRecord::new(
            &parent.id,
            now,
            TranscriptEvent::SubagentSettled {
                child_id: child.child_id.clone(),
                status: status.label().to_string(),
            },
        ))?;

        Ok(child)
    }

    /// Removes a child from messaging and observation.
    ///
    /// A tombstone is written and the transcript on disk is left exactly where
    /// it is. "Delete" here means "stop addressing", never "destroy evidence".
    pub fn delete(&self, parent_selector: &str, selector: &str) -> Result<Subagent> {
        let parent = self.sessions.resolve(parent_selector)?;
        let mut child = self.registry.get(&parent.id, selector)?;
        child.settle(SubagentStatus::Deleted, self.env.now());
        self.registry.update(&child)?;
        Ok(child)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;
    use crate::sessions::{SessionService, StartSession};

    struct Fixture {
        subagents: SubagentService,
        sessions_service: SessionService,
        sessions: Arc<MemSessions>,
        runner: Arc<MemRunner>,
        transcript: Arc<MemTranscript>,
    }

    fn fixture(max_depth: u8) -> Fixture {
        let (env, _clock) = env();
        let sessions = Arc::new(MemSessions::default());
        let registry = Arc::new(MemSubagents::default());
        let runner = Arc::new(MemRunner::ready());
        let runners = Arc::new(MemRunnerRegistry::new(runner.clone()));
        let transcript = Arc::new(MemTranscript::default());
        Fixture {
            subagents: SubagentService::new(
                env.clone(),
                sessions.clone(),
                registry,
                runners,
                transcript.clone(),
                DepthPolicy { max_depth },
            ),
            sessions_service: SessionService::new(
                env,
                sessions.clone(),
                transcript.clone(),
                Arc::new(MemSupervisor::default()),
            ),
            sessions,
            runner,
            transcript,
        }
    }

    fn root(fixture: &Fixture) -> Session {
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

    fn spawn(fixture: &Fixture, parent: &str, name: &str) -> Result<SpawnHandle> {
        fixture.subagents.spawn(Spawn {
            parent_selector: parent.into(),
            prompt: "review the API".into(),
            name: Some(name.into()),
            model: None,
            runner: None,
            system_prompt: None,
        })
    }

    fn call(fixture: &Fixture, parent: &str, name: &str) -> Result<CallResult> {
        fixture.subagents.call(
            Spawn {
                parent_selector: parent.into(),
                prompt: "is this a breaking change?".into(),
                name: Some(name.into()),
                model: None,
                runner: None,
                system_prompt: None,
            },
            None,
        )
    }

    #[test]
    fn a_call_waits_and_comes_back_with_the_answer() {
        let fixture = fixture(1);
        let parent = root(&fixture);

        let result = call(&fixture, &parent.id, "reviewer").unwrap();

        // The whole difference from `spawn`: there is an answer here.
        assert!(result.ok);
        assert_eq!(result.text, "ran: is this a breaking change?");

        // And it really ran in the foreground. A detached run would have come
        // back with a pid and no text at all.
        let requests = fixture.runner.calls.lock().unwrap();
        assert!(!requests[0].detached);
    }

    #[test]
    fn a_call_settles_its_child_and_charges_the_parent_once() {
        let fixture = fixture(1);
        let parent = root(&fixture);

        call(&fixture, &parent.id, "reviewer").unwrap();

        let child = fixture.subagents.get(&parent.id, "reviewer").unwrap();
        assert_eq!(child.status, SubagentStatus::Completed);
        assert!(child.usage_attributed);

        // Waiting for a child is exactly when its cost is known, so there is
        // no reason to make the caller run `settle` by hand afterwards.
        let parent_session = fixture.sessions.get(&parent.id).unwrap();
        assert_eq!(parent_session.usage.attributed_child_tokens, 15);
    }

    #[test]
    fn a_child_that_ran_and_failed_says_so_rather_than_answering_emptily() {
        let fixture = fixture(1);
        let parent = root(&fixture);
        *fixture.runner.reply.lock().unwrap() = Some(RunOutcome::failure("model refused", Some(1)));

        let result = call(&fixture, &parent.id, "reviewer").unwrap();

        // An empty string that reads like an answer is the failure mode this
        // guards against: `ok` is what a caller branches on.
        assert!(!result.ok);
        assert_eq!(result.error.as_deref(), Some("model refused"));

        let child = fixture.subagents.get(&parent.id, "reviewer").unwrap();
        assert_eq!(child.status, SubagentStatus::Failed);
        assert_eq!(child.last_error.as_deref(), Some("model refused"));
    }

    #[test]
    fn a_call_obeys_the_same_depth_limit_as_a_spawn() {
        let fixture = fixture(0);
        let parent = root(&fixture);

        // Blocking is not a way around the recursion limit.
        assert!(call(&fixture, &parent.id, "reviewer").is_err());
        assert!(fixture.sessions.list().unwrap().len() == 1);
    }

    #[test]
    fn spawning_returns_an_admission_handle_not_an_answer() {
        let fixture = fixture(1);
        root(&fixture);
        let handle = spawn(&fixture, "root", "api-reviewer").unwrap();

        assert_eq!(handle.name, "api-reviewer");
        assert_eq!(handle.depth, 1);
        assert!(!handle.child_id.is_empty());
        assert!(!handle.session_dir.is_empty());
        // The runner was asked to work detached; nothing waited for a reply.
        assert!(fixture.runner.calls.lock().unwrap()[0].detached);
    }

    #[test]
    fn a_child_inherits_the_parents_model_and_runner() {
        let fixture = fixture(1);
        root(&fixture);
        let handle = spawn(&fixture, "root", "api-reviewer").unwrap();
        assert_eq!(handle.model, "sonnet");
        let child = fixture.subagents.get("root", "api-reviewer").unwrap();
        assert_eq!(child.runner, "claude");
    }

    #[test]
    fn depth_stops_grandchildren_before_anything_is_created() {
        let fixture = fixture(1);
        root(&fixture);
        spawn(&fixture, "root", "child").unwrap();

        let before = fixture.sessions.list().unwrap().len();
        assert!(matches!(
            spawn(&fixture, "child", "grandchild"),
            Err(DomainError::Forbidden(_))
        ));
        assert_eq!(fixture.sessions.list().unwrap().len(), before);
    }

    #[test]
    fn raising_the_depth_allows_a_grandchild() {
        let fixture = fixture(2);
        root(&fixture);
        spawn(&fixture, "root", "child").unwrap();
        assert_eq!(spawn(&fixture, "child", "grandchild").unwrap().depth, 2);
    }

    #[test]
    fn two_children_cannot_share_a_name() {
        let fixture = fixture(1);
        root(&fixture);
        spawn(&fixture, "root", "reviewer").unwrap();
        assert!(matches!(
            spawn(&fixture, "root", "reviewer"),
            Err(DomainError::Conflict { .. })
        ));
    }

    #[test]
    fn a_child_that_fails_to_launch_is_recorded_not_forgotten() {
        let fixture = fixture(1);
        root(&fixture);
        *fixture.runner.available.lock().unwrap() = false;

        assert!(spawn(&fixture, "root", "reviewer").is_err());

        // The failure is visible to the parent rather than vanishing: a
        // delegation that never ran is exactly the thing it needs to notice.
        let child = fixture.subagents.get("root", "reviewer").unwrap();
        assert_eq!(child.status, SubagentStatus::Failed);
        assert!(child.last_error.is_some());
        assert!(fixture
            .transcript
            .summaries()
            .iter()
            .any(|line| line.contains("error in spawn")));
    }

    #[test]
    fn child_usage_is_attributed_to_the_parent_exactly_once() {
        let fixture = fixture(1);
        let parent = root(&fixture);
        spawn(&fixture, "root", "reviewer").unwrap();

        let usage = Usage {
            input_tokens: 100,
            output_tokens: 50,
            turns: 1,
            attributed_child_tokens: 0,
        };
        fixture
            .subagents
            .settle("root", "reviewer", SubagentStatus::Completed, usage)
            .unwrap();
        fixture
            .subagents
            .settle("root", "reviewer", SubagentStatus::Completed, usage)
            .unwrap();

        let parent = fixture.sessions.get(&parent.id).unwrap();
        assert_eq!(parent.usage.total_tokens(), 150);
        assert_eq!(parent.usage.own_tokens(), 0);

        let attributions = fixture
            .transcript
            .summaries()
            .into_iter()
            .filter(|line| line.contains("attributed"))
            .count();
        assert_eq!(attributions, 1);
    }

    #[test]
    fn a_deleted_child_keeps_its_transcript_but_loses_its_address() {
        let fixture = fixture(1);
        root(&fixture);
        spawn(&fixture, "root", "reviewer").unwrap();
        let deleted = fixture.subagents.delete("root", "reviewer").unwrap();

        assert_eq!(deleted.status, SubagentStatus::Deleted);
        assert!(!deleted.status.is_addressable());
        assert!(fixture.subagents.list("root", false).unwrap().is_empty());
        assert_eq!(fixture.subagents.list("root", true).unwrap().len(), 1);
    }

    #[test]
    fn completed_children_remain_addressable() {
        let fixture = fixture(1);
        root(&fixture);
        spawn(&fixture, "root", "reviewer").unwrap();
        fixture
            .subagents
            .settle("root", "reviewer", SubagentStatus::Completed, Usage::default())
            .unwrap();
        assert!(fixture
            .subagents
            .get("root", "reviewer")
            .unwrap()
            .status
            .is_addressable());
    }
}
