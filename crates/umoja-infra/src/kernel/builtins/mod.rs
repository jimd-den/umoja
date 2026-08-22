//! Native Rhai built-in function extensions.

pub mod dataset;
pub mod files;
pub mod lsp;

use rhai::Engine;

/// Registers all native built-in functions into a Rhai engine.
pub fn register_all(engine: &mut Engine) {
    dataset::register_dataset_builtins(engine);
    files::register_files_builtins(engine);
    lsp::register_lsp_builtins(engine);
}
