/// Identifier minting, as a dependency.
///
/// Ids are prefixed (`ses-`, `sub-`, `job-`, `ref-`) so that a bare string in a
/// log or an error message says what kind of thing it names.
pub trait IdGen: Send + Sync {
    fn next(&self, prefix: &str) -> String;
}

/// The prefixes in use. Centralised so a typo cannot silently create a second
/// namespace of, say, jobs.
pub struct Ids;

impl Ids {
    pub const SESSION: &'static str = "ses";
    pub const SUBAGENT: &'static str = "sub";
    pub const JOB: &'static str = "job";
    pub const HEARTBEAT: &'static str = "hb";
    pub const ENTRY: &'static str = "ent";
    pub const REFINEMENT: &'static str = "ref";
    pub const MESSAGE: &'static str = "msg";
}

impl std::fmt::Debug for Ids {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Ids")
    }
}

/// A deterministic generator for tests: `pfx-000001`, `pfx-000002`, ...
#[derive(Debug, Default)]
pub struct SeqIdGen(std::sync::atomic::AtomicU64);

impl IdGen for SeqIdGen {
    fn next(&self, prefix: &str) -> String {
        let n = self
            .0
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            .saturating_add(1);
        format!("{prefix}-{n:06}")
    }
}
