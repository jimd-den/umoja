//! Prompt-as-a-variable: the persistent namespace, as a use case.
//!
//! The service is thin on purpose. Its whole job is to make the kernel lazy,
//! bounded and recorded — start it only when it is used, clip what it prints,
//! and write down that it ran.

use std::sync::Arc;

use umoja_domain::prelude::*;
use umoja_domain::transcript::{TranscriptEvent, TranscriptRecord};

use crate::Env;

pub struct KernelService {
    env: Env,
    kernel: Arc<dyn KernelPort>,
    transcript: Arc<dyn TranscriptLog>,
}

impl std::fmt::Debug for KernelService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KernelService")
    }
}

impl KernelService {
    pub fn new(env: Env, kernel: Arc<dyn KernelPort>, transcript: Arc<dyn TranscriptLog>) -> Self {
        Self {
            env,
            kernel,
            transcript,
        }
    }

    pub fn language(&self) -> KernelLanguage {
        self.kernel.language()
    }

    pub fn ensure(&self, session_id: &str) -> Result<KernelStatus> {
        self.kernel.ensure(session_id)
    }

    pub fn status(&self, session_id: &str) -> Result<KernelStatus> {
        self.kernel.status(session_id)
    }

    pub fn execute(&self, request: ExecRequest) -> Result<ExecOutcome> {
        self.kernel.ensure(&request.session_id)?;
        let outcome = self.kernel.execute(&request)?.clip(request.max_output_bytes);

        self.transcript.append(&TranscriptRecord::new(
            &request.session_id,
            self.env.now(),
            TranscriptEvent::KernelExec {
                code: request.code.clone(),
                ok: outcome.ok,
                duration_ms: outcome.duration_ms,
            },
        ))?;

        Ok(outcome)
    }

    /// Names and shapes, never values — see [`umoja_domain::kernel::VarSummary`].
    ///
    /// Sorted biggest first, because the reason anybody runs this is to find
    /// out what is eating memory or what is worth slicing.
    pub fn vars(&self, session_id: &str) -> Result<Vec<VarSummary>> {
        let mut vars = self.kernel.vars(session_id)?;
        vars.sort_by(|a, b| {
            b.size_bytes
                .unwrap_or(0)
                .cmp(&a.size_bytes.unwrap_or(0))
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(vars)
    }

    pub fn reset(&self, session_id: &str) -> Result<()> {
        self.kernel.reset(session_id)
    }

    pub fn shutdown(&self, session_id: &str) -> Result<()> {
        self.kernel.shutdown(session_id)
    }

    pub fn snapshot(&self, session_id: &str) -> Result<Option<String>> {
        self.kernel.snapshot(session_id)
    }

    pub fn restore(&self, session_id: &str) -> Result<bool> {
        self.kernel.restore(session_id)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::doubles::*;

    fn service() -> (KernelService, Arc<MemKernel>, Arc<MemTranscript>) {
        let (env, _clock) = env();
        let kernel = Arc::new(MemKernel::default());
        let transcript = Arc::new(MemTranscript::default());
        (
            KernelService::new(env, kernel.clone(), transcript.clone()),
            kernel,
            transcript,
        )
    }

    #[test]
    fn the_namespace_survives_between_calls() {
        let (service, _, _) = service();
        service
            .execute(ExecRequest::new("ses-1", "rows = 4200000").unwrap())
            .unwrap();
        let outcome = service
            .execute(ExecRequest::new("ses-1", "rows").unwrap())
            .unwrap();
        assert_eq!(outcome.stdout, "4200000");
    }

    #[test]
    fn the_kernel_starts_lazily_but_only_once() {
        let (service, kernel, _) = service();
        assert_eq!(service.status("ses-1").unwrap(), KernelStatus::Cold);
        service
            .execute(ExecRequest::new("ses-1", "a = 1").unwrap())
            .unwrap();
        assert!(*kernel.started.lock().unwrap());
    }

    #[test]
    fn runaway_output_is_clipped_not_dropped() {
        let (service, kernel, _) = service();
        kernel
            .execute(&ExecRequest::new("ses-1", format!("big = {}", "x".repeat(5000))).unwrap())
            .unwrap();
        let outcome = service
            .execute(ExecRequest::new("ses-1", "big").unwrap().with_max_output(512))
            .unwrap();
        assert!(outcome.truncated_bytes > 0);
        assert!(outcome.stdout.contains("clipped"));
    }

    #[test]
    fn every_execution_is_recorded() {
        let (service, _, transcript) = service();
        service
            .execute(ExecRequest::new("ses-1", "a = 1").unwrap())
            .unwrap();
        service
            .execute(ExecRequest::new("ses-1", "nope").unwrap())
            .unwrap();
        let lines = transcript.summaries();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("ok"));
        assert!(lines[1].contains("failed"));
    }

    #[test]
    fn a_failed_lookup_is_an_outcome_not_an_error() {
        let (service, _, _) = service();
        let outcome = service
            .execute(ExecRequest::new("ses-1", "missing").unwrap())
            .unwrap();
        assert!(!outcome.ok);
        assert!(outcome.error.unwrap().contains("NameError"));
    }

    #[test]
    fn vars_report_shape_and_are_ordered_by_size() {
        let (service, _, _) = service();
        service
            .execute(ExecRequest::new("ses-1", "small = ab").unwrap())
            .unwrap();
        service
            .execute(ExecRequest::new("ses-1", "large = abcdefghij").unwrap())
            .unwrap();
        let vars = service.vars("ses-1").unwrap();
        assert_eq!(vars[0].name, "large");
        assert_eq!(vars[1].name, "small");
    }

    #[test]
    fn reset_empties_the_namespace() {
        let (service, _, _) = service();
        service
            .execute(ExecRequest::new("ses-1", "a = 1").unwrap())
            .unwrap();
        service.reset("ses-1").unwrap();
        assert!(service.vars("ses-1").unwrap().is_empty());
    }
}
