//! Clock, ids and process supervision — the small pieces of "the world".

use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use pa_domain::clock::Clock;
use pa_domain::error::{DomainError, Result};
use pa_domain::ids::IdGen;
use pa_domain::ports::ProcessSupervisor;

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// Time-ordered, collision-resistant, and readable in a log.
///
/// `ses-mfk2p91x-0007-a3f1`: a base-36 timestamp so ids sort chronologically,
/// a per-process counter so a fast loop cannot repeat, and entropy from the
/// process id and address space so two machines writing to one shared home do
/// not collide either.
#[derive(Debug)]
pub struct TimeOrderedIds {
    counter: AtomicU64,
    salt: u64,
}

impl Default for TimeOrderedIds {
    fn default() -> Self {
        let pid = std::process::id() as u64;
        let stack_entropy = &pid as *const u64 as u64;
        Self {
            counter: AtomicU64::new(0),
            salt: pid ^ (stack_entropy >> 8),
        }
    }
}

impl IdGen for TimeOrderedIds {
    fn next(&self, prefix: &str) -> String {
        let micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_micros() as u64)
            .unwrap_or_default();
        let sequence = self.counter.fetch_add(1, Ordering::Relaxed);
        format!(
            "{prefix}-{}-{:04}-{:04x}",
            radix36(micros),
            sequence % 10_000,
            (self.salt.wrapping_add(sequence)) & 0xFFFF
        )
    }
}

fn radix36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".into();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

/// POSIX process control, via signals sent by `kill`.
#[derive(Debug, Default)]
pub struct UnixProcessSupervisor;

impl ProcessSupervisor for UnixProcessSupervisor {
    fn is_alive(&self, pid: u32) -> bool {
        // Signal 0 performs the permission and existence checks without
        // delivering anything, which is exactly the question being asked.
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    fn terminate(&self, pid: u32, force: bool) -> Result<()> {
        // TERM first: a worker that can flush its transcript should be allowed
        // to. KILL is reserved for an explicit --force.
        let signal = if force { "-KILL" } else { "-TERM" };
        let status = Command::new("kill")
            .args([signal, &pid.to_string()])
            .status()
            .map_err(|error| DomainError::adapter("kill", error))?;
        if !status.success() && self.is_alive(pid) {
            return Err(DomainError::adapter(
                "kill",
                format!("could not stop process {pid}"),
            ));
        }
        Ok(())
    }

    fn current_pid(&self) -> u32 {
        std::process::id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn ids_are_unique_and_sort_chronologically() {
        let ids = TimeOrderedIds::default();
        let minted: Vec<String> = (0..1000).map(|_| ids.next("ses")).collect();
        let unique: HashSet<&String> = minted.iter().collect();
        assert_eq!(unique.len(), minted.len());

        let mut sorted = minted.clone();
        sorted.sort();
        assert_eq!(sorted.first(), minted.first());
    }

    #[test]
    fn ids_carry_their_prefix() {
        assert!(TimeOrderedIds::default().next("job").starts_with("job-"));
    }

    #[test]
    fn the_current_process_is_alive_and_pid_one_is_not_ours_to_kill() {
        let supervisor = UnixProcessSupervisor;
        assert!(supervisor.is_alive(supervisor.current_pid()));
        // A pid that cannot exist on any Linux system.
        assert!(!supervisor.is_alive(4_294_967_294));
    }
}
