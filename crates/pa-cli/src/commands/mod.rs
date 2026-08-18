//! Command handlers.
//!
//! Each one reads arguments, calls exactly one use case, and renders the
//! result. No handler contains a rule: if something here looks like a decision,
//! it belongs in `pa-app` instead.

mod lifecycle;
mod plans;
mod work;

use pa_domain::error::Result;

use crate::cli::{Cli, Command};
use crate::output::Output;
use crate::wiring::App;

pub fn dispatch(cli: &Cli, app: &App) -> Result<Output> {
    match &cli.command {
        Command::Start(args) => lifecycle::start(app, args),
        Command::Agents(args) => lifecycle::agents(app, args),
        Command::Rename { selector, name } => lifecycle::rename(app, selector, name),
        Command::Stop { selector, force } => lifecycle::stop(app, selector, *force),
        Command::Status => lifecycle::status(app),
        Command::Doctor { fix } => lifecycle::doctor(app, *fix),
        Command::Shutdown { force } => lifecycle::shutdown(app, *force),
        Command::Log { lines, follow } => lifecycle::log(app, *lines, *follow),
        Command::Attach {
            selector,
            no_follow,
            lines,
        } => lifecycle::attach(app, selector.as_deref(), *lines, !*no_follow),
        Command::Tick { dry_run } => lifecycle::tick(app, *dry_run),

        Command::Kernel(command) => work::kernel(app, command),
        Command::Harness(command) => work::harness(app, command),
        Command::Refine(command) => work::refine(app, command),
        Command::Agent(command) => work::agent(app, command),
        Command::Send(args) => work::send(app, args),
        Command::Inbox { consume } => work::inbox(app, *consume),
        Command::Roster => work::roster(app),

        Command::Goal(command) => plans::goal(app, command),
        Command::Heartbeat(command) => plans::heartbeat(app, command),
        Command::Schedule(command) => plans::schedule(app, command),
        Command::Autonomous(command) => plans::autonomous(app, command),
        Command::Compact(command) => plans::compact(app, command),
        Command::Skills(command) => plans::skills(app, command),
        Command::Prompt => plans::prompt(app),
    }
}
