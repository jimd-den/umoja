//! Native Rhai built-in function extensions.

pub mod dataset;
pub mod files;

use rhai::Engine;

/// Registers all native built-in functions into a Rhai engine.
pub fn register_all(engine: &mut Engine) {
    dataset::register_dataset_builtins(engine);
    files::register_files_builtins(engine);
}
