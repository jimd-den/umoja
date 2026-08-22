//! Kernels: the persistent execution namespace.
//!
//! [`rhai_kernel::RhaiKernel`] provides an in-process, pure Rust embedded
//! execution kernel that persists variable state across separate CLI invocations.
//! [`shell::ShellKernel`] persists working directory and exported environment variables.
//! [`socket::SocketKernel`] optionally supports socket interpreters.

pub mod builtins;
pub mod rhai_kernel;
pub mod shell;
pub mod socket;

use std::sync::Arc;

use umoja_domain::error::Result;
use umoja_domain::kernel::KernelLanguage;
use umoja_domain::ports::KernelPort;

use crate::paths::Paths;

/// Builds the kernel for a language, defaulting to in-process pure Rust Rhai.
pub fn build(
    language: KernelLanguage,
    paths: Paths,
    default_cwd: String,
) -> Result<Arc<dyn KernelPort>> {
    Ok(match language {
        KernelLanguage::Rhai => Arc::new(rhai_kernel::RhaiKernel::new().with_paths(paths)),
        KernelLanguage::Shell => Arc::new(shell::ShellKernel::new(paths, default_cwd)),
        other => Arc::new(socket::SocketKernel::new(
            paths,
            socket::KernelConfig::for_language(other),
        )),
    })
}
