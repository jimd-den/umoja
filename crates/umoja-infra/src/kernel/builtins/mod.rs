//! Native Rhai built-in function extensions.

pub mod astgrep;
pub mod dataset;
pub mod files;
pub mod lsp;
pub mod reporting;

use rhai::Engine;

/// Registers all native built-in functions into a Rhai engine.
pub fn register_all(engine: &mut Engine) {
    astgrep::register_astgrep_builtins(engine);
    dataset::register_dataset_builtins(engine);
    files::register_files_builtins(engine);
    lsp::register_lsp_builtins(engine);
    reporting::register_reporting_builtins(engine);
}
