//! Kernels: the persistent execution namespace, in three flavours.
//!
//! [`socket::SocketKernel`] holds a real interpreter process (Python or Node)
//! behind a Unix socket, which is what makes a variable survive between two
//! separate `pa` invocations. [`shell::ShellKernel`] persists what a shell can
//! honestly persist — a working directory and exported variables — without a
//! daemon at all.

pub mod shell;
pub mod socket;

use std::sync::Arc;

use pa_domain::error::Result;
use pa_domain::kernel::KernelLanguage;
use pa_domain::ports::KernelPort;

use crate::paths::Paths;

/// Builds the kernel for a language, so the composition root does not have to
/// know that one of them is shaped differently from the others.
pub fn build(
    language: KernelLanguage,
    paths: Paths,
    default_cwd: String,
) -> Result<Arc<dyn KernelPort>> {
    Ok(match language {
        KernelLanguage::Shell => Arc::new(shell::ShellKernel::new(paths, default_cwd)),
        other => Arc::new(socket::SocketKernel::new(
            paths,
            socket::KernelConfig::for_language(other),
        )),
    })
}
