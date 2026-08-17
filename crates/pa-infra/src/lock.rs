//! A cross-process advisory lock.
//!
//! `pa` is a one-shot binary: a cron tick, an interactive command and a
//! subagent settling can all be running at the same moment against the same
//! registry file. Atomic rename keeps each write whole, but read-modify-write
//! still needs a lock or the later writer silently discards the earlier one.
//!
//! This is an exclusive-create lockfile — the portable approach that needs no
//! dependencies. A lock older than [`STALE_AFTER`] is broken on the assumption
//! its holder died, which is the right trade for a tool where a stuck lock
//! would block every later command.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use pa_domain::error::{DomainError, Result};

use crate::paths::ensure_parent;

const STALE_AFTER: Duration = Duration::from_secs(30);
const WAIT_LIMIT: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_millis(20);

#[derive(Debug)]
pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    pub fn acquire(target: &Path) -> Result<Self> {
        let path = lock_path(target);
        ensure_parent(&path)?;
        let started = Instant::now();

        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut file) => {
                    let _ = write!(file, "{}", std::process::id());
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if Self::is_stale(&path) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if started.elapsed() > WAIT_LIMIT {
                        return Err(DomainError::adapter(
                            format!("lock {}", target.display()),
                            "another prime-agent process is holding this file",
                        ));
                    }
                    std::thread::sleep(RETRY_DELAY);
                }
                Err(error) => {
                    return Err(DomainError::adapter(
                        format!("lock {}", target.display()),
                        error,
                    ))
                }
            }
        }
    }

    fn is_stale(path: &Path) -> bool {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .map(|modified| {
                SystemTime::now()
                    .duration_since(modified)
                    .map(|age| age > STALE_AFTER)
                    .unwrap_or(false)
            })
            .unwrap_or(true)
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn lock_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "target".into());
    target.with_file_name(format!(".{name}.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pa-lock-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_lock_is_released_when_it_is_dropped() {
        let dir = tempdir("release");
        let target = dir.join("registry.json");
        {
            let _lock = FileLock::acquire(&target).unwrap();
            assert!(lock_path(&target).exists());
        }
        assert!(!lock_path(&target).exists());
        // And can be taken again immediately.
        let _second = FileLock::acquire(&target).unwrap();
    }

    #[test]
    fn a_stale_lock_is_broken_rather_than_blocking_forever() {
        let dir = tempdir("stale");
        let target = dir.join("registry.json");
        let held = lock_path(&target);
        std::fs::write(&held, b"99999").unwrap();

        // Backdate the lock past the staleness horizon.
        let old = SystemTime::now() - Duration::from_secs(120);
        let file = std::fs::OpenOptions::new().write(true).open(&held).unwrap();
        file.set_modified(old).unwrap();

        let _lock = FileLock::acquire(&target).unwrap();
    }
}
