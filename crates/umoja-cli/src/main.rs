//! `pa` — Prime Agent's capabilities as a portable tool.
//!
//! The binary itself does four things: parse, wire, dispatch, and choose an
//! exit code. Everything else lives in a layer that can be tested without a
//! process.

#![forbid(unsafe_code)]

mod cli;
mod commands;
mod output;
mod wiring;

use clap::Parser;
use umoja_domain::error::DomainError;

use crate::cli::Cli;
use crate::wiring::App;

/// Exit codes, so a script can branch without parsing prose.
pub(crate) mod exit {
    /// The command ran and its answer was negative — a failing gate, a dead
    /// kernel, code that raised. Distinct from a broken invocation.
    pub const NEGATIVE: i32 = 1;
    /// Autonomous mode wants another turn.
    pub const CONTINUE: i32 = 2;
    /// Bad input: a name that does not exist, an interval that does not parse.
    pub const USAGE: i32 = 64;
    /// The state forbids it: a completed goal being resumed.
    pub const FORBIDDEN: i32 = 65;
    /// Something is not installed or not supported here.
    pub const UNSUPPORTED: i32 = 69;
    /// The world failed: disk, process, interpreter.
    pub const ADAPTER: i32 = 70;
}

fn main() {
    let cli = Cli::parse();

    let app = match App::build(&cli) {
        Ok(app) => app,
        Err(error) => {
            report(&error);
            std::process::exit(code_for(&error));
        }
    };

    match commands::dispatch(&cli, &app) {
        Ok(output) => {
            output.print(cli.json);
            if output.code != 0 {
                std::process::exit(output.code);
            }
        }
        Err(error) => {
            if cli.json {
                let payload = serde_json::json!({
                    "error": error.to_string(),
                    "kind": kind_of(&error),
                    "transient": error.is_transient(),
                });
                println!(
                    "{}",
                    serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".into())
                );
            } else {
                report(&error);
            }
            std::process::exit(code_for(&error));
        }
    }
}

fn report(error: &DomainError) {
    eprintln!("pa: {error}");
    // A hint only where one exists and is actually actionable.
    if let DomainError::NotFound { kind, .. } = error {
        if *kind == "session" {
            eprintln!("     `pa agents` lists what is there; `pa start <name>` makes a new one.");
        }
    }
}

fn kind_of(error: &DomainError) -> &'static str {
    match error {
        DomainError::Invalid(_) => "invalid",
        DomainError::NotFound { .. } => "not_found",
        DomainError::Conflict { .. } => "conflict",
        DomainError::Forbidden(_) => "forbidden",
        DomainError::LimitReached { .. } => "limit_reached",
        DomainError::Adapter { .. } => "adapter",
        DomainError::Parse { .. } => "parse",
        DomainError::Unsupported(_) => "unsupported",
    }
}

fn code_for(error: &DomainError) -> i32 {
    match error {
        DomainError::Invalid(_) | DomainError::NotFound { .. } | DomainError::Parse { .. } => {
            exit::USAGE
        }
        DomainError::Conflict { .. }
        | DomainError::Forbidden(_)
        | DomainError::LimitReached { .. } => exit::FORBIDDEN,
        DomainError::Unsupported(_) => exit::UNSUPPORTED,
        DomainError::Adapter { .. } => exit::ADAPTER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_tree_is_well_formed() {
        // clap validates the whole definition here: duplicate flags, bad
        // defaults and conflicting aliases all fail this one assertion.
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn every_error_has_a_distinct_exit_code_family() {
        assert_eq!(code_for(&DomainError::invalid("x")), exit::USAGE);
        assert_eq!(code_for(&DomainError::not_found("session", "x")), exit::USAGE);
        assert_eq!(code_for(&DomainError::forbidden("x")), exit::FORBIDDEN);
        assert_eq!(
            code_for(&DomainError::Unsupported("x".into())),
            exit::UNSUPPORTED
        );
        assert_eq!(code_for(&DomainError::adapter("x", "y")), exit::ADAPTER);
        assert_ne!(exit::NEGATIVE, exit::CONTINUE);
    }

    #[test]
    fn error_kinds_are_stable_names_for_scripts() {
        assert_eq!(kind_of(&DomainError::invalid("x")), "invalid");
        assert_eq!(kind_of(&DomainError::conflict("session", "x")), "conflict");
        assert!(DomainError::adapter("x", "y").is_transient());
        assert!(!DomainError::invalid("x").is_transient());
    }
}
