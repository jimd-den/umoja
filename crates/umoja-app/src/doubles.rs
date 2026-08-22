//! In-memory implementations of every port, for testing use cases.
//!
//! These are not mocks that assert on calls; they are working implementations
//! with the same observable behaviour as the real adapters. A use-case test
//! that passes against these is testing the rule, not the plumbing.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use umoja_domain::clock::Clock;
use umoja_domain::ids::SeqIdGen;
use umoja_domain::prelude::*;
use umoja_domain::transcript::TranscriptRecord;

use crate::Env;

pub fn at(text: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(text)
        .unwrap()
        .with_timezone(&Utc)
}

/// A clock that can be moved forward by a test.
#[derive(Debug)]
pub struct TestClock(Mutex<DateTime<Utc>>);

impl TestClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self(Mutex::new(start))
    }

    pub fn advance_secs(&self, secs: i64) {
        let mut guard = self.0.lock().unwrap();
        *guard += chrono::Duration::seconds(secs);
    }

    pub fn set(&self, to: DateTime<Utc>) {
        *self.0.lock().unwrap() = to;
    }
}

impl Clock for TestClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

pub fn env() -> (Env, Arc<TestClock>) {
    let clock = Arc::new(TestClock::new(at("2026-08-16T12:00:00Z")));
    let env = Env::new(clock.clone(), Arc::new(SeqIdGen::default()));
    (env, clock)
}

macro_rules! lock {
    ($self:expr, $field:ident) => {
        $self.$field.lock().unwrap()
    };
}

// --- sessions ---------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemSessions {
    rows: Mutex<Vec<Session>>,
}

impl SessionStore for MemSessions {
    fn insert(&self, session: &Session) -> Result<()> {
        let mut rows = lock!(self, rows);
        if rows.iter().any(|row| row.id == session.id) {
            return Err(DomainError::conflict("session", &session.id));
        }
        if rows.iter().any(|row| row.name == session.name) {
            return Err(DomainError::conflict("session name", &session.name));
        }
        rows.push(session.clone());
        Ok(())
    }

    fn update(&self, session: &Session) -> Result<()> {
        let mut rows = lock!(self, rows);
        let slot = rows
            .iter_mut()
            .find(|row| row.id == session.id)
            .ok_or_else(|| DomainError::not_found("session", &session.id))?;
        *slot = session.clone();
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Session> {
        lock!(self, rows)
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .ok_or_else(|| DomainError::not_found("session", id))
    }

    fn resolve(&self, selector: &str) -> Result<Session> {
        let rows = lock!(self, rows);
        let hits: Vec<&Session> = rows
            .iter()
            .filter(|row| row.matches_selector(selector))
            .collect();
        match hits.len() {
            0 => Err(DomainError::not_found("session", selector)),
            1 => Ok(hits[0].clone()),
            _ => Err(DomainError::conflict("session selector", selector)),
        }
    }

    fn list(&self) -> Result<Vec<Session>> {
        Ok(lock!(self, rows).clone())
    }

    fn children_of(&self, parent_id: &str) -> Result<Vec<Session>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| row.parent_id.as_deref() == Some(parent_id))
            .cloned()
            .collect())
    }

    fn remove(&self, id: &str) -> Result<()> {
        lock!(self, rows).retain(|row| row.id != id);
        Ok(())
    }
}

// --- transcript -------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemTranscript {
    pub rows: Mutex<Vec<TranscriptRecord>>,
}

impl MemTranscript {
    pub fn summaries(&self) -> Vec<String> {
        lock!(self, rows).iter().map(|row| row.summary()).collect()
    }
}

impl TranscriptLog for MemTranscript {
    fn append(&self, record: &TranscriptRecord) -> Result<()> {
        lock!(self, rows).push(record.clone());
        Ok(())
    }

    fn read(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<TranscriptRecord>> {
        let rows = lock!(self, rows);
        let mut hits: Vec<TranscriptRecord> = rows
            .iter()
            .filter(|row| row.session_id == session_id)
            .cloned()
            .collect();
        if let Some(limit) = limit {
            let start = hits.len().saturating_sub(limit);
            hits = hits.split_off(start);
        }
        Ok(hits)
    }
}

// --- harness ----------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemHarness {
    entries: Mutex<Vec<(Option<String>, HarnessEntry)>>,
    refinements: Mutex<Vec<Refinement>>,
}

fn scope_key(session_id: Option<&str>, scope: HarnessScope) -> Option<String> {
    match scope {
        HarnessScope::Global => None,
        HarnessScope::Local => session_id.map(str::to_string),
    }
}

impl HarnessStore for MemHarness {
    fn upsert(&self, session_id: Option<&str>, entry: &HarnessEntry) -> Result<()> {
        let key = scope_key(session_id, entry.scope);
        let mut rows = lock!(self, entries);
        if let Some(slot) = rows
            .iter_mut()
            .find(|(k, e)| *k == key && e.name == entry.name && e.scope == entry.scope)
        {
            slot.1 = entry.clone();
        } else {
            rows.push((key, entry.clone()));
        }
        Ok(())
    }

    fn get(&self, session_id: Option<&str>, scope: HarnessScope, name: &str) -> Result<HarnessEntry> {
        let key = scope_key(session_id, scope);
        lock!(self, entries)
            .iter()
            .find(|(k, e)| *k == key && e.name == name && e.scope == scope)
            .map(|(_, e)| e.clone())
            .ok_or_else(|| DomainError::not_found("harness entry", name))
    }

    fn remove(&self, session_id: Option<&str>, scope: HarnessScope, name: &str) -> Result<()> {
        let key = scope_key(session_id, scope);
        let mut rows = lock!(self, entries);
        let before = rows.len();
        rows.retain(|(k, e)| !(*k == key && e.name == name && e.scope == scope));
        if rows.len() == before {
            return Err(DomainError::not_found("harness entry", name));
        }
        Ok(())
    }

    fn list(&self, session_id: Option<&str>) -> Result<Vec<HarnessEntry>> {
        Ok(lock!(self, entries)
            .iter()
            .filter(|(k, e)| {
                e.scope == HarnessScope::Global || k.as_deref() == session_id
            })
            .map(|(_, e)| e.clone())
            .collect())
    }

    fn record_refinement(&self, _session_id: Option<&str>, refinement: &Refinement) -> Result<()> {
        lock!(self, refinements).push(refinement.clone());
        Ok(())
    }

    fn update_refinement(&self, _session_id: Option<&str>, refinement: &Refinement) -> Result<()> {
        let mut rows = lock!(self, refinements);
        let slot = rows
            .iter_mut()
            .find(|row| row.id == refinement.id)
            .ok_or_else(|| DomainError::not_found("refinement", &refinement.id))?;
        *slot = refinement.clone();
        Ok(())
    }

    fn refinements(
        &self,
        _session_id: Option<&str>,
        limit: Option<usize>,
    ) -> Result<Vec<Refinement>> {
        let rows = lock!(self, refinements);
        let mut hits = rows.clone();
        hits.reverse();
        if let Some(limit) = limit {
            hits.truncate(limit);
        }
        Ok(hits)
    }

    fn refinement(&self, _session_id: Option<&str>, id: &str) -> Result<Refinement> {
        lock!(self, refinements)
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .ok_or_else(|| DomainError::not_found("refinement", id))
    }
}

// --- goals ------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemGoals {
    rows: Mutex<Vec<Goal>>,
}

impl GoalStore for MemGoals {
    fn put(&self, goal: &Goal) -> Result<()> {
        let mut rows = lock!(self, rows);
        match rows.iter_mut().find(|row| row.session_id == goal.session_id) {
            Some(slot) => *slot = goal.clone(),
            None => rows.push(goal.clone()),
        }
        Ok(())
    }

    fn get(&self, session_id: &str) -> Result<Option<Goal>> {
        Ok(lock!(self, rows)
            .iter()
            .find(|row| row.session_id == session_id)
            .cloned())
    }

    fn clear(&self, session_id: &str) -> Result<()> {
        lock!(self, rows).retain(|row| row.session_id != session_id);
        Ok(())
    }

    fn active(&self) -> Result<Vec<Goal>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| row.should_continue())
            .cloned()
            .collect())
    }
}

// --- heartbeats -------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemHeartbeats {
    rows: Mutex<Vec<Heartbeat>>,
}

impl HeartbeatStore for MemHeartbeats {
    fn put(&self, heartbeat: &Heartbeat) -> Result<()> {
        let mut rows = lock!(self, rows);
        match rows.iter_mut().find(|row| row.id == heartbeat.id) {
            Some(slot) => *slot = heartbeat.clone(),
            None => rows.push(heartbeat.clone()),
        }
        Ok(())
    }

    fn get(&self, id: &str) -> Result<Heartbeat> {
        lock!(self, rows)
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .ok_or_else(|| DomainError::not_found("heartbeat", id))
    }

    fn remove(&self, id: &str) -> Result<()> {
        lock!(self, rows).retain(|row| row.id != id);
        Ok(())
    }

    fn list(&self, session_id: Option<&str>) -> Result<Vec<Heartbeat>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| session_id.is_none_or(|id| row.session_id == id))
            .cloned()
            .collect())
    }

    fn due(&self, now: DateTime<Utc>) -> Result<Vec<Heartbeat>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| row.is_due(now))
            .cloned()
            .collect())
    }
}

// --- schedules --------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemSchedules {
    rows: Mutex<Vec<ScheduledJob>>,
}

impl ScheduleStore for MemSchedules {
    fn put(&self, job: &ScheduledJob) -> Result<()> {
        let mut rows = lock!(self, rows);
        match rows.iter_mut().find(|row| row.id == job.id) {
            Some(slot) => *slot = job.clone(),
            None => rows.push(job.clone()),
        }
        Ok(())
    }

    fn get(&self, id: &str) -> Result<ScheduledJob> {
        lock!(self, rows)
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .ok_or_else(|| DomainError::not_found("job", id))
    }

    fn list(&self, target: Option<&str>, include_finished: bool) -> Result<Vec<ScheduledJob>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| target.is_none_or(|t| row.target == t))
            .filter(|row| {
                include_finished
                    || matches!(row.status, JobStatus::Pending | JobStatus::Claimed)
            })
            .cloned()
            .collect())
    }

    fn due(&self, now: DateTime<Utc>) -> Result<Vec<ScheduledJob>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| row.is_due(now))
            .cloned()
            .collect())
    }

    fn remove(&self, id: &str) -> Result<()> {
        lock!(self, rows).retain(|row| row.id != id);
        Ok(())
    }
}

// --- messages ---------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemMessages {
    rows: Mutex<Vec<AgentMessage>>,
}

impl MessageStore for MemMessages {
    fn enqueue(&self, message: &AgentMessage) -> Result<()> {
        lock!(self, rows).push(message.clone());
        Ok(())
    }

    fn update(&self, message: &AgentMessage) -> Result<()> {
        let mut rows = lock!(self, rows);
        let slot = rows
            .iter_mut()
            .find(|row| row.id == message.id)
            .ok_or_else(|| DomainError::not_found("message", &message.id))?;
        *slot = message.clone();
        Ok(())
    }

    fn pending_for(&self, session_id: &str) -> Result<Vec<AgentMessage>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| row.receiver_session_id == session_id && row.is_pending())
            .cloned()
            .collect())
    }

    fn outbox(&self, session_id: &str, limit: Option<usize>) -> Result<Vec<AgentMessage>> {
        let rows = lock!(self, rows);
        let mut hits: Vec<AgentMessage> = rows
            .iter()
            .filter(|row| row.sender_session_id == session_id)
            .cloned()
            .collect();
        hits.reverse();
        if let Some(limit) = limit {
            hits.truncate(limit);
        }
        Ok(hits)
    }

    fn get(&self, id: &str) -> Result<AgentMessage> {
        lock!(self, rows)
            .iter()
            .find(|row| row.id == id)
            .cloned()
            .ok_or_else(|| DomainError::not_found("message", id))
    }
}

// --- subagents --------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemSubagents {
    rows: Mutex<Vec<Subagent>>,
}

impl SubagentRegistry for MemSubagents {
    fn insert(&self, child: &Subagent) -> Result<()> {
        let mut rows = lock!(self, rows);
        if rows
            .iter()
            .any(|row| row.parent_session_id == child.parent_session_id && row.name == child.name)
        {
            return Err(DomainError::conflict("subagent name", &child.name));
        }
        rows.push(child.clone());
        Ok(())
    }

    fn update(&self, child: &Subagent) -> Result<()> {
        let mut rows = lock!(self, rows);
        let slot = rows
            .iter_mut()
            .find(|row| row.child_id == child.child_id)
            .ok_or_else(|| DomainError::not_found("subagent", &child.child_id))?;
        *slot = child.clone();
        Ok(())
    }

    fn get(&self, parent_session_id: &str, selector: &str) -> Result<Subagent> {
        lock!(self, rows)
            .iter()
            .find(|row| row.parent_session_id == parent_session_id && row.matches_selector(selector))
            .cloned()
            .ok_or_else(|| DomainError::not_found("subagent", selector))
    }

    fn list(&self, parent_session_id: &str, include_deleted: bool) -> Result<Vec<Subagent>> {
        Ok(lock!(self, rows)
            .iter()
            .filter(|row| row.parent_session_id == parent_session_id)
            .filter(|row| include_deleted || row.status != SubagentStatus::Deleted)
            .cloned()
            .collect())
    }

    fn all(&self) -> Result<Vec<Subagent>> {
        Ok(lock!(self, rows).clone())
    }
}

// --- autonomous / compaction ------------------------------------------------

#[derive(Debug, Default)]
pub struct MemAutonomous {
    rows: Mutex<Vec<AutonomousState>>,
}

impl AutonomousStore for MemAutonomous {
    fn put(&self, state: &AutonomousState) -> Result<()> {
        let mut rows = lock!(self, rows);
        match rows.iter_mut().find(|row| row.session_id == state.session_id) {
            Some(slot) => *slot = state.clone(),
            None => rows.push(state.clone()),
        }
        Ok(())
    }

    fn get(&self, session_id: &str) -> Result<Option<AutonomousState>> {
        Ok(lock!(self, rows)
            .iter()
            .find(|row| row.session_id == session_id)
            .cloned())
    }

    fn clear(&self, session_id: &str) -> Result<()> {
        lock!(self, rows).retain(|row| row.session_id != session_id);
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct MemCompaction {
    rows: Mutex<Vec<CompactionState>>,
}

impl CompactionStore for MemCompaction {
    fn put(&self, state: &CompactionState) -> Result<()> {
        let mut rows = lock!(self, rows);
        match rows.iter_mut().find(|row| row.session_id == state.session_id) {
            Some(slot) => *slot = state.clone(),
            None => rows.push(state.clone()),
        }
        Ok(())
    }

    fn get(&self, session_id: &str) -> Result<Option<CompactionState>> {
        Ok(lock!(self, rows)
            .iter()
            .find(|row| row.session_id == session_id)
            .cloned())
    }
}

// --- kernel -----------------------------------------------------------------

/// A namespace of string bindings. Enough to prove the use cases: `a = 1`
/// binds, `a` recalls, anything else is an error.
#[derive(Debug, Default)]
pub struct MemKernel {
    vars: Mutex<Vec<(String, String)>>,
    pub started: Mutex<bool>,
}

impl KernelPort for MemKernel {
    fn language(&self) -> KernelLanguage {
        KernelLanguage::Python
    }

    fn status(&self, _session_id: &str) -> Result<KernelStatus> {
        Ok(if *lock!(self, started) {
            KernelStatus::Ready
        } else {
            KernelStatus::Cold
        })
    }

    fn ensure(&self, _session_id: &str) -> Result<KernelStatus> {
        *lock!(self, started) = true;
        Ok(KernelStatus::Ready)
    }

    fn execute(&self, request: &ExecRequest) -> Result<ExecOutcome> {
        *lock!(self, started) = true;
        let code = request.code.trim();
        let mut vars = lock!(self, vars);
        if let Some((name, value)) = code.split_once('=') {
            let (name, value) = (name.trim().to_string(), value.trim().to_string());
            vars.retain(|(existing, _)| *existing != name);
            vars.push((name, value));
            return Ok(ExecOutcome {
                ok: true,
                stdout: String::new(),
                stderr: String::new(),
                result: None,
                error: None,
                duration_ms: 1,
                truncated_bytes: 0,
                timed_out: false,
            });
        }
        match vars.iter().find(|(name, _)| name == code) {
            Some((_, value)) => Ok(ExecOutcome {
                ok: true,
                stdout: value.clone(),
                stderr: String::new(),
                result: Some(value.clone()),
                error: None,
                duration_ms: 1,
                truncated_bytes: 0,
                timed_out: false,
            }),
            None => Ok(ExecOutcome::failure(format!("NameError: {code}"), 1)),
        }
    }

    fn vars(&self, _session_id: &str) -> Result<Vec<VarSummary>> {
        Ok(lock!(self, vars)
            .iter()
            .map(|(name, value)| VarSummary {
                name: name.clone(),
                type_name: "str".into(),
                length: Some(value.len() as u64),
                size_bytes: Some(value.len() as u64),
                preview: None,
            })
            .collect())
    }

    fn reset(&self, _session_id: &str) -> Result<()> {
        lock!(self, vars).clear();
        Ok(())
    }

    fn shutdown(&self, _session_id: &str) -> Result<()> {
        *lock!(self, started) = false;
        Ok(())
    }

    fn snapshot(&self, _session_id: &str) -> Result<Option<String>> {
        Ok(None)
    }

    fn restore(&self, _session_id: &str) -> Result<bool> {
        Ok(false)
    }
}

// --- runner -----------------------------------------------------------------

/// Records what it was asked to run and replies with a canned answer.
#[derive(Debug, Default)]
pub struct MemRunner {
    pub calls: Mutex<Vec<RunRequest>>,
    pub reply: Mutex<Option<RunOutcome>>,
    pub available: Mutex<bool>,
}

impl MemRunner {
    pub fn ready() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            reply: Mutex::new(None),
            available: Mutex::new(true),
        }
    }

    pub fn prompts(&self) -> Vec<String> {
        lock!(self, calls)
            .iter()
            .map(|call| call.prompt.clone())
            .collect()
    }
}

impl AgentRunner for MemRunner {
    fn capabilities(&self) -> RunnerCapabilities {
        RunnerCapabilities {
            name: "memory".into(),
            can_resume: true,
            can_stream: false,
            reports_usage: true,
            supports_system_prompt: true,
            supports_model_selection: true,
        }
    }

    fn run(&self, request: &RunRequest) -> Result<RunOutcome> {
        lock!(self, calls).push(request.clone());
        if !*lock!(self, available) {
            return Err(DomainError::adapter("memory runner", "not installed"));
        }
        if let Some(reply) = lock!(self, reply).clone() {
            return Ok(reply);
        }
        Ok(RunOutcome {
            ok: true,
            text: format!("ran: {}", request.prompt),
            usage: Usage {
                input_tokens: 10,
                output_tokens: 5,
                turns: 1,
                attributed_child_tokens: 0,
            },
            runner_session: Some("runner-1".into()),
            exit_code: Some(0),
            error: None,
            pid: None,
            duration_ms: 5,
        })
    }

    fn probe(&self) -> Result<()> {
        if *lock!(self, available) {
            Ok(())
        } else {
            Err(DomainError::Unsupported("memory runner is off".into()))
        }
    }
}

/// Hands out one runner for every name, and records which names were asked
/// for so a test can assert that the session's own harness was used.
#[derive(Debug)]
pub struct MemRunnerRegistry {
    pub runner: Arc<MemRunner>,
    pub asked: Mutex<Vec<String>>,
}

impl MemRunnerRegistry {
    pub fn new(runner: Arc<MemRunner>) -> Self {
        Self {
            runner,
            asked: Mutex::new(Vec::new()),
        }
    }
}

impl RunnerRegistry for MemRunnerRegistry {
    fn get(&self, name: &str) -> Result<Arc<dyn AgentRunner>> {
        lock!(self, asked).push(name.to_string());
        Ok(self.runner.clone())
    }

    fn default_name(&self) -> String {
        "memory".into()
    }
}

// --- gates ------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct MemGates {
    pub results: Mutex<Vec<(String, bool)>>,
    pub fingerprint: Mutex<Option<String>>,
    pub runs: Mutex<Vec<String>>,
}

impl GateRunner for MemGates {
    fn run(&self, gate: &Gate, _workdir: &str) -> Result<GateOutcome> {
        lock!(self, runs).push(gate.command.clone());
        let passed = lock!(self, results)
            .iter()
            .find(|(command, _)| *command == gate.command)
            .map(|(_, passed)| *passed)
            .unwrap_or(true);
        Ok(GateOutcome {
            command: gate.command.clone(),
            passed,
            exit_code: Some(if passed { 0 } else { 1 }),
            output: if passed { String::new() } else { "boom".into() },
            workspace_fingerprint: lock!(self, fingerprint).clone(),
            ran_at: at("2026-08-16T12:00:00Z"),
        })
    }

    fn fingerprint(&self, _workdir: &str) -> Result<Option<String>> {
        Ok(lock!(self, fingerprint).clone())
    }
}

// --- skills / summariser / supervisor ---------------------------------------

#[derive(Debug, Default)]
pub struct MemSkills {
    pub manifests: Mutex<Vec<SkillManifest>>,
}

impl SkillCatalog for MemSkills {
    fn discover(&self, _workdir: &str) -> Result<Vec<SkillManifest>> {
        Ok(lock!(self, manifests).clone())
    }

    fn load_body(&self, manifest: &SkillManifest) -> Result<String> {
        Ok(format!("# {}\n\nbody", manifest.name))
    }
}

#[derive(Debug, Default)]
pub struct MemSummariser;

impl Summariser for MemSummariser {
    fn summarise(&self, plan: &CompactionPlan, records: &[TranscriptRecord]) -> Result<String> {
        Ok(format!(
            "summary of {} records ({})",
            records.len(),
            plan.trigger.label()
        ))
    }
}

#[derive(Debug, Default)]
pub struct MemSupervisor {
    pub alive: Mutex<Vec<u32>>,
    pub terminated: Mutex<Vec<u32>>,
}

impl ProcessSupervisor for MemSupervisor {
    fn is_alive(&self, pid: u32) -> bool {
        lock!(self, alive).contains(&pid)
    }

    fn terminate(&self, pid: u32, _force: bool) -> Result<()> {
        lock!(self, terminated).push(pid);
        lock!(self, alive).retain(|row| *row != pid);
        Ok(())
    }

    fn current_pid(&self) -> u32 {
        4242
    }
}
