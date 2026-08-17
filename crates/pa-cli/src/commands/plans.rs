//! Goals, heartbeats, schedules, autonomy, compaction and skills.

use pa_app::heartbeats::CreateHeartbeat;
use pa_app::schedules::AddJob;
use pa_domain::prelude::*;
use serde_json::json;

use crate::cli::{
    AutonomousCommand, CompactCommand, GoalCommand, HeartbeatArgs, HeartbeatCommand,
    ScheduleCommand, SkillsCommand,
};
use crate::exit;
use crate::output::{ago, clip, duration, table, Output};
use crate::wiring::App;

pub fn goal(app: &App, command: &GoalCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        GoalCommand::Set {
            objective,
            budget,
            deadline,
            replace,
        } => {
            let wall_clock_secs = deadline
                .as_deref()
                .map(Interval::parse)
                .transpose()?
                .map(|interval| interval.as_secs());

            let goal = app.goals.create(
                &session.id,
                objective,
                GoalBudget {
                    tokens: *budget,
                    wall_clock_secs,
                    continuations: None,
                },
                *replace,
            )?;

            Ok(Output::new(
                format!("goal set: {}", goal.objective),
                describe_goal(&goal),
            ))
        }

        GoalCommand::Status => match app.goals.get(&session.id)? {
            Some(goal) => {
                let mut lines = vec![
                    format!("{} [{}]", goal.objective, goal.status.label()),
                    format!(
                        "spent {} tokens over {} and {} continuations",
                        goal.progress.tokens_used,
                        duration(goal.progress.elapsed_secs as i64),
                        goal.progress.continuations
                    ),
                ];
                if let Some(remaining) = goal.remaining_tokens() {
                    lines.push(format!("{remaining} tokens of budget left"));
                }
                if let Some(note) = &goal.note {
                    lines.push(note.clone());
                }
                Ok(Output::new(lines.join("\n"), describe_goal(&goal)))
            }
            None => Ok(Output::new("no goal", json!({ "goal": null }))),
        },

        GoalCommand::Pause => {
            let goal = app.goals.pause(&session.id)?;
            Ok(Output::new("goal paused", describe_goal(&goal)))
        }
        GoalCommand::Resume => {
            let goal = app.goals.resume(&session.id)?;
            Ok(Output::new("goal resumed", describe_goal(&goal)))
        }
        GoalCommand::Complete => {
            let goal = app.goals.complete(&session.id)?;
            Ok(Output::new("goal complete", describe_goal(&goal)))
        }
        GoalCommand::Clear => {
            app.goals.clear(&session.id)?;
            Ok(Output::message("goal cleared"))
        }
    }
}

fn describe_goal(goal: &Goal) -> serde_json::Value {
    json!({
        "objective": goal.objective,
        "status": goal.status.label(),
        "tokens_used": goal.progress.tokens_used,
        "continuations": goal.progress.continuations,
        "elapsed_secs": goal.progress.elapsed_secs,
        "budget_tokens": goal.budget.tokens,
        "remaining_tokens": goal.remaining_tokens(),
        "note": goal.note,
    })
}

pub fn heartbeat(app: &App, command: &HeartbeatCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        HeartbeatCommand::Set(args) => create(app, &session, args, HeartbeatOwner::User),
        HeartbeatCommand::Add(args) => create(app, &session, args, HeartbeatOwner::Agent),

        HeartbeatCommand::List { all } => {
            let now = app.env.now();
            let scope = if *all { None } else { Some(session.id.as_str()) };
            let beats = app.heartbeats.list(scope)?;

            let rows: Vec<Vec<String>> = beats
                .iter()
                .map(|beat| {
                    vec![
                        beat.id.clone(),
                        match beat.owner {
                            HeartbeatOwner::User => "user".to_string(),
                            HeartbeatOwner::Agent => "agent".to_string(),
                        },
                        beat.interval.to_string(),
                        match beat.status {
                            HeartbeatStatus::Active => ago(beat.next_fire_at, now),
                            HeartbeatStatus::Paused => "paused".to_string(),
                        },
                        clip(&beat.prompt, 44),
                    ]
                })
                .collect();

            Ok(Output::new(
                if rows.is_empty() {
                    "no heartbeats".to_string()
                } else {
                    table(&["id", "owner", "every", "next", "prompt"], &rows)
                },
                json!({ "heartbeats": beats }),
            ))
        }

        // The user is the actor at the CLI, so they may manage an agent's
        // heartbeats as well as their own. The reverse is what the domain
        // forbids, and that guard lives in the service.
        HeartbeatCommand::Pause { id } => {
            let beat = app.heartbeats.pause(id, HeartbeatOwner::User)?;
            Ok(Output::new("heartbeat paused", json!({ "id": beat.id })))
        }
        HeartbeatCommand::Resume { id } => {
            let beat = app.heartbeats.resume(id, HeartbeatOwner::User)?;
            Ok(Output::new(
                format!("heartbeat resumed; next in {}", beat.interval),
                json!({ "id": beat.id, "next_fire_at": beat.next_fire_at.to_rfc3339() }),
            ))
        }
        HeartbeatCommand::Remove { id } => {
            app.heartbeats.remove(id, HeartbeatOwner::User)?;
            Ok(Output::message("heartbeat removed"))
        }
        HeartbeatCommand::Clear => {
            let cleared = app.heartbeats.clear_user(&session.id)?;
            Ok(Output::new(
                if cleared {
                    "heartbeat cleared"
                } else {
                    "there was no user heartbeat"
                },
                json!({ "cleared": cleared }),
            ))
        }
    }
}

fn create(
    app: &App,
    session: &Session,
    args: &HeartbeatArgs,
    owner: HeartbeatOwner,
) -> Result<Output> {
    let beat = app.heartbeats.create(CreateHeartbeat {
        selector: session.id.clone(),
        prompt: args.prompt.clone(),
        interval: Interval::parse(&args.every)?,
        owner,
        label: args.label.clone(),
        delivery: DeliveryMode::parse(&args.mode)?,
    })?;

    Ok(Output::new(
        format!("heartbeat {} every {}", beat.id, beat.interval),
        json!({
            "id": beat.id,
            "interval": beat.interval.to_string(),
            "next_fire_at": beat.next_fire_at.to_rfc3339(),
            "owner": if owner == HeartbeatOwner::User { "user" } else { "agent" },
        }),
    ))
}

pub fn schedule(app: &App, command: &ScheduleCommand) -> Result<Output> {
    match command {
        ScheduleCommand::Add {
            target,
            when,
            prompt,
            mode,
        } => {
            let job = app.schedules.add(AddJob {
                target: target.clone(),
                when: when.clone(),
                prompt: prompt.clone(),
                delivery: DeliveryMode::parse(mode)?,
            })?;

            Ok(Output::new(
                format!(
                    "{} scheduled {} ({})",
                    job.id,
                    job.spec.describe(),
                    job.next_tick
                        .map(|tick| tick.to_rfc3339())
                        .unwrap_or_else(|| "never".into())
                ),
                json!({
                    "id": job.id,
                    "target": job.target,
                    "spec": job.spec.describe(),
                    "next_tick": job.next_tick.map(|tick| tick.to_rfc3339()),
                }),
            ))
        }

        ScheduleCommand::List { all, target } => {
            let now = app.env.now();
            let jobs = app.schedules.list(target.as_deref(), *all)?;

            let rows: Vec<Vec<String>> = jobs
                .iter()
                .map(|job| {
                    vec![
                        job.id.clone(),
                        job.target.clone(),
                        job.spec.describe(),
                        job.next_tick
                            .map(|tick| ago(tick, now))
                            .unwrap_or_else(|| format!("{:?}", job.status).to_lowercase()),
                        clip(&job.prompt, 40),
                    ]
                })
                .collect();

            Ok(Output::new(
                if rows.is_empty() {
                    "nothing scheduled".to_string()
                } else {
                    table(&["id", "target", "when", "next", "prompt"], &rows)
                },
                json!({ "jobs": jobs }),
            ))
        }

        ScheduleCommand::Cancel { id } => {
            let job = app.schedules.cancel(id)?;
            Ok(Output::new(
                format!("cancelled {}", job.id),
                json!({ "id": job.id }),
            ))
        }
    }
}

pub fn autonomous(app: &App, command: &AutonomousCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        AutonomousCommand::On {
            gates,
            max_continuations,
            max_turns,
            max_tokens,
            max_time,
        } => {
            let policy = AutonomousPolicy {
                enabled: true,
                gates: gates
                    .iter()
                    .map(|command| Gate::new(command.clone()))
                    .collect::<Result<Vec<_>>>()?,
                limits: AutonomousLimits {
                    max_continuations: *max_continuations,
                    max_turns: *max_turns,
                    max_tokens: *max_tokens,
                    max_wall_clock_secs: Interval::parse(max_time)?.as_secs(),
                },
            };

            let state = app.autonomy.enable(&session.id, policy)?;
            Ok(Output::new(
                format!(
                    "autonomous mode on with {} gate(s); limits {} turns, {} tokens, {}",
                    state.policy.gates.len(),
                    state.policy.limits.max_turns,
                    state.policy.limits.max_tokens,
                    duration(state.policy.limits.max_wall_clock_secs as i64)
                ),
                json!({ "enabled": true, "gates": state.policy.gates }),
            ))
        }

        AutonomousCommand::Off => {
            app.autonomy.disable(&session.id)?;
            Ok(Output::message("autonomous mode off"))
        }

        AutonomousCommand::Status => match app.autonomy.status(&session.id)? {
            Some(state) => {
                let now = app.env.now();
                let decision = state.decide(now);
                let mut lines = vec![
                    format!("{} — {}", decision.label(), decision.reason()),
                    format!(
                        "{} continuations, {} turns, {} tokens, {} elapsed",
                        state.continuations,
                        state.turns,
                        state.usage.total_tokens(),
                        duration(state.elapsed_secs(now) as i64)
                    ),
                ];
                for gate in &state.last_gate_outcomes {
                    lines.push(format!(
                        "gate {} {}",
                        if gate.passed { "pass" } else { "FAIL" },
                        gate.command
                    ));
                }
                Ok(Output::new(
                    lines.join("\n"),
                    json!({
                        "decision": decision.label(),
                        "reason": decision.reason(),
                        "continuations": state.continuations,
                        "turns": state.turns,
                        "tokens": state.usage.total_tokens(),
                        "gates": state.last_gate_outcomes,
                    }),
                ))
            }
            None => Ok(Output::new("autonomous mode is off", json!({ "enabled": false }))),
        },

        AutonomousCommand::Step => {
            let report = app.autonomy.step(&session.id)?;
            let mut lines = vec![format!(
                "{} — {}",
                report.decision.label(),
                report.decision.reason()
            )];
            for gate in &report.gates {
                lines.push(format!(
                    "gate {} {}",
                    if gate.passed { "pass" } else { "FAIL" },
                    gate.command
                ));
            }
            for skipped in &report.skipped_gates {
                lines.push(format!("gate skip {skipped} (workspace unchanged)"));
            }
            if let Some(prompt) = app.autonomy.continuation_prompt(&report) {
                lines.push(String::new());
                lines.push(prompt);
            }

            // Exit 2 means "keep going" so a shell loop can drive the run:
            //   while pa autonomous step; [ $? -eq 2 ]; do ...; done
            let code = if report.decision.should_continue() {
                exit::CONTINUE
            } else {
                0
            };

            Ok(Output::new(
                lines.join("\n"),
                json!({
                    "decision": report.decision.label(),
                    "reason": report.decision.reason(),
                    "should_continue": report.decision.should_continue(),
                    "gates": report.gates,
                    "skipped_gates": report.skipped_gates,
                    "continuation_prompt": app.autonomy.continuation_prompt(&report),
                }),
            )
            .with_code(code))
        }
    }
}

pub fn compact(app: &App, command: &CompactCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        CompactCommand::Status => {
            let state = app.compaction.status(&session.id)?;
            Ok(Output::new(
                format!(
                    "{} of {} tokens ({:.0}% of the window); {}",
                    state.used_tokens,
                    state.context_window,
                    state.utilisation() * 100.0,
                    if state.is_due() {
                        "compaction is due"
                    } else {
                        "no compaction needed"
                    }
                ),
                json!({
                    "used_tokens": state.used_tokens,
                    "context_window": state.context_window,
                    "utilisation": state.utilisation(),
                    "due": state.is_due(),
                    "compactions": state.compactions,
                }),
            ))
        }

        CompactCommand::Run { instruction } => {
            let report = app.compaction.run(
                &session.id,
                CompactionTrigger::Manual,
                instruction.clone(),
            )?;
            Ok(Output::new(
                if report.records_summarised == 0 {
                    "nothing old enough to compact".to_string()
                } else {
                    report.summary.clone()
                },
                json!({
                    "summary": report.summary,
                    "records_summarised": report.records_summarised,
                    "used_tokens": report.state.used_tokens,
                }),
            ))
        }
    }
}

pub fn skills(app: &App, command: &SkillsCommand) -> Result<Output> {
    match command {
        SkillsCommand::List => {
            let index = app.skills.index(&app.workdir)?;
            let rows: Vec<Vec<String>> = index
                .skills
                .iter()
                .map(|skill| {
                    vec![
                        skill.name.clone(),
                        skill.source.label().to_string(),
                        match skill.kind {
                            SkillKind::Markdown => "md".to_string(),
                            SkillKind::Executable => "exec".to_string(),
                        },
                        clip(&skill.description, 56),
                    ]
                })
                .collect();

            let mut text = if rows.is_empty() {
                "no skills found".to_string()
            } else {
                table(&["name", "source", "kind", "description"], &rows)
            };
            for note in &index.notes {
                text.push_str(&format!("\nnote: {note}"));
            }

            Ok(Output::new(
                text,
                json!({ "skills": index.skills, "notes": index.notes }),
            ))
        }

        SkillsCommand::Show { name } => {
            let body = app.skills.body(&app.workdir, name)?;
            Ok(Output::new(body.clone(), json!({ "name": name, "body": body })))
        }

        SkillsCommand::Prompt => {
            let block = app.skills.prompt_block(&app.workdir)?;
            Ok(Output::new(block.clone(), json!({ "prompt": block })))
        }
    }
}

/// The whole supplemental prompt: harness, skills and any live goal.
///
/// One command so a host agent can splice a single block into its context
/// rather than making three calls and deciding how to join them.
pub fn prompt(app: &App) -> Result<Output> {
    let session = app.session()?;

    let harness = app.harness.prompt_block(Some(&session.id))?;
    let skills = app.skills.prompt_block(&app.workdir)?;
    let goal = app
        .goals
        .get(&session.id)?
        .and_then(|goal| app.goals.continuation_prompt(&goal))
        .unwrap_or_default();

    let block = [harness.as_str(), skills.as_str(), goal.as_str()]
        .iter()
        .filter(|part| !part.trim().is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("\n\n");

    Ok(Output::new(
        block.clone(),
        json!({
            "prompt": block,
            "harness": harness,
            "skills": skills,
            "goal": goal,
        }),
    ))
}
