//! Kernel, harness, refinement, subagents and messaging.

use std::io::Read;

use pa_app::harness::Remember;
use pa_app::messaging::Send;
use pa_app::subagents::Spawn;
use pa_domain::prelude::*;
use serde_json::json;

use crate::cli::{
    AgentCommand, HarnessCommand, KernelCommand, RefineCommand, SendArgs, SpawnArgs,
};
use crate::exit;
use crate::output::{clip, table, Output};
use crate::wiring::App;

pub fn kernel(app: &App, command: &KernelCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        KernelCommand::Exec {
            code,
            timeout,
            lang,
            max_output,
        } => {
            let source = if code == "-" {
                // Reading from stdin is what makes a heredoc or a generated
                // script usable without shell-quoting a program.
                let mut buffer = String::new();
                std::io::stdin()
                    .read_to_string(&mut buffer)
                    .map_err(|error| DomainError::adapter("read stdin", error))?;
                buffer
            } else {
                code.clone()
            };

            let kernel = app.kernel(lang.as_deref())?;
            let outcome = kernel.execute(
                ExecRequest::new(&session.id, source)?
                    .with_timeout(*timeout)
                    .with_max_output(*max_output),
            )?;

            let mut text = String::new();
            if !outcome.stdout.trim().is_empty() {
                text.push_str(outcome.stdout.trim_end());
            }
            if let Some(result) = &outcome.result {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(result);
            }
            if !outcome.stderr.trim().is_empty() {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(outcome.stderr.trim_end());
            }
            if let Some(error) = &outcome.error {
                if !text.is_empty() {
                    text.push('\n');
                }
                text.push_str(error);
            }

            Ok(Output::new(
                text,
                json!({
                    "ok": outcome.ok,
                    "stdout": outcome.stdout,
                    "stderr": outcome.stderr,
                    "result": outcome.result,
                    "error": outcome.error,
                    "duration_ms": outcome.duration_ms,
                    "truncated_bytes": outcome.truncated_bytes,
                    "timed_out": outcome.timed_out,
                }),
            )
            .with_code(if outcome.ok { 0 } else { exit::NEGATIVE }))
        }

        KernelCommand::Vars { lang } => {
            let vars = app.kernel(lang.as_deref())?.vars(&session.id)?;
            let rows: Vec<Vec<String>> = vars
                .iter()
                .map(|var| {
                    vec![
                        var.name.clone(),
                        var.type_name.clone(),
                        var.length.map(|n| n.to_string()).unwrap_or_else(|| "-".into()),
                        var.size_bytes
                            .map(human_bytes)
                            .unwrap_or_else(|| "-".into()),
                        clip(var.preview.as_deref().unwrap_or(""), 48),
                    ]
                })
                .collect();

            Ok(Output::new(
                if rows.is_empty() {
                    "namespace is empty".to_string()
                } else {
                    table(&["name", "type", "len", "size", "preview"], &rows)
                },
                json!({ "vars": vars }),
            ))
        }

        KernelCommand::Reset { lang } => {
            app.kernel(lang.as_deref())?.reset(&session.id)?;
            Ok(Output::message("namespace cleared"))
        }

        KernelCommand::Status { lang } => {
            let kernel = app.kernel(lang.as_deref())?;
            let status = kernel.status(&session.id)?;
            let label = format!("{status:?}").to_lowercase();
            Ok(Output::new(
                format!("{} kernel is {label}", kernel.language().label()),
                json!({ "language": kernel.language().label(), "status": label }),
            )
            .with_code(if status == KernelStatus::Dead { exit::NEGATIVE } else { 0 }))
        }

        KernelCommand::Stop { lang } => {
            app.kernel(lang.as_deref())?.shutdown(&session.id)?;
            Ok(Output::message("kernel stopped"))
        }

        KernelCommand::Snapshot { lang } => {
            let path = app.kernel(lang.as_deref())?.snapshot(&session.id)?;
            Ok(match path {
                Some(path) => Output::new(
                    format!("snapshot written to {path}"),
                    json!({ "path": path }),
                ),
                None => Output::new(
                    "nothing to snapshot; the kernel is not running",
                    json!({ "path": null }),
                ),
            })
        }

        KernelCommand::Restore { lang } => {
            let restored = app.kernel(lang.as_deref())?.restore(&session.id)?;
            Ok(Output::new(
                if restored {
                    "namespace restored"
                } else {
                    "no snapshot to restore"
                },
                json!({ "restored": restored }),
            ))
        }
    }
}

fn human_bytes(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b}B"),
        b if b < 1024 * 1024 => format!("{:.1}K", b as f64 / 1024.0),
        b if b < 1024 * 1024 * 1024 => format!("{:.1}M", b as f64 / (1024.0 * 1024.0)),
        b => format!("{:.1}G", b as f64 / (1024.0 * 1024.0 * 1024.0)),
    }
}

pub fn harness(app: &App, command: &HarnessCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        HarnessCommand::List { kind } => {
            let filter = kind.as_deref().map(EntryKind::parse).transpose()?;
            let entries = app.harness.list(Some(&session.id), filter)?;

            let rows: Vec<Vec<String>> = entries
                .iter()
                .map(|entry| {
                    vec![
                        entry.kind.label().to_string(),
                        entry.scope.label().to_string(),
                        entry.name.clone(),
                        clip(&entry.body, 56),
                    ]
                })
                .collect();

            Ok(Output::new(
                if rows.is_empty() {
                    "the harness is empty".to_string()
                } else {
                    table(&["kind", "scope", "name", "body"], &rows)
                },
                json!({ "entries": entries }),
            ))
        }

        HarnessCommand::Show { name } => {
            let entries = app.harness.list(Some(&session.id), None)?;
            let normalised = HarnessEntry::normalise_name(name)?;
            let entry = entries
                .into_iter()
                .find(|entry| entry.name == normalised)
                .ok_or_else(|| DomainError::not_found("harness entry", name))?;

            let mut text = format!(
                "{} [{}/{}]\n\n{}",
                entry.name,
                entry.kind.label(),
                entry.scope.label(),
                entry.body
            );
            text.push_str(&format!("\n\nevidence: {}", entry.evidence));
            if let Some(outcome) = &entry.outcome {
                text.push_str(&format!("\noutcome:  {outcome}"));
            }

            Ok(Output::new(text, serde_json::to_value(&entry).unwrap_or(json!({}))))
        }

        HarnessCommand::Remember(args) => {
            let (entry, refinement) = app.harness.remember(Remember {
                session_id: Some(session.id.clone()),
                kind: EntryKind::parse(&args.kind)?,
                scope: HarnessScope::parse(&args.scope)?,
                name: args.name.clone(),
                body: args.body.clone(),
                evidence: args.evidence.clone(),
                outcome: args.outcome.clone(),
                tags: args.tags.clone(),
            })?;

            Ok(Output::new(
                format!(
                    "{} {} '{}' ({})",
                    refinement.op.label(),
                    entry.kind.label(),
                    entry.name,
                    refinement.id
                ),
                json!({ "entry": entry, "refinement_id": refinement.id, "op": refinement.op.label() }),
            ))
        }

        HarnessCommand::Forget {
            name,
            scope,
            evidence,
        } => {
            let refinement = app.harness.forget(
                Some(&session.id),
                HarnessScope::parse(scope)?,
                name,
                evidence,
            )?;
            Ok(Output::new(
                format!("forgot '{name}' ({}) — undo with: pa refine rollback {}", refinement.op.label(), refinement.id),
                json!({ "refinement_id": refinement.id }),
            ))
        }

        HarnessCommand::Prompt => {
            let block = app.harness.prompt_block(Some(&session.id))?;
            Ok(Output::new(block.clone(), json!({ "prompt": block })))
        }
    }
}

pub fn refine(app: &App, command: &RefineCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        RefineCommand::List { limit, global } => {
            let scope = if *global { None } else { Some(session.id.as_str()) };
            let refinements = app.harness.refinements(scope, Some(*limit))?;

            let rows: Vec<Vec<String>> = refinements
                .iter()
                .map(|refinement| {
                    vec![
                        refinement.id.clone(),
                        refinement.op.label().to_string(),
                        if refinement.is_reverted() {
                            "reverted".to_string()
                        } else {
                            "applied".to_string()
                        },
                        clip(&refinement.summary, 52),
                    ]
                })
                .collect();

            Ok(Output::new(
                if rows.is_empty() {
                    "nothing has been refined yet".to_string()
                } else {
                    table(&["id", "op", "state", "summary"], &rows)
                },
                json!({ "refinements": refinements }),
            ))
        }

        RefineCommand::Show { id } => {
            let refinement = app
                .harness
                .refinements(Some(&session.id), None)?
                .into_iter()
                .find(|row| row.id == *id)
                .ok_or_else(|| DomainError::not_found("refinement", id))?;

            let mut text = format!(
                "{} [{}]\n{}\n\nevidence: {}",
                refinement.id,
                refinement.op.label(),
                refinement.summary,
                refinement.evidence
            );
            if let Some(before) = &refinement.snapshot.before {
                text.push_str(&format!("\n\nbefore:\n{}", before.body));
            }
            if let Some(after) = &refinement.snapshot.after {
                text.push_str(&format!("\n\nafter:\n{}", after.body));
            }
            if let Some(by) = &refinement.reverted_by {
                text.push_str(&format!("\n\nrolled back by {by}"));
            }

            Ok(Output::new(
                text,
                serde_json::to_value(&refinement).unwrap_or(json!({})),
            ))
        }

        RefineCommand::Rollback { id } => {
            let undo = app.harness.rollback(Some(&session.id), id)?;
            Ok(Output::new(
                format!("rolled back {id} ({})", undo.id),
                json!({ "rollback_id": undo.id, "of": id }),
            ))
        }
    }
}

pub fn agent(app: &App, command: &AgentCommand) -> Result<Output> {
    let session = app.session()?;

    match command {
        AgentCommand::Spawn(args) => spawn(app, &session, args),

        AgentCommand::List { all } => {
            let children = app.subagents.list(&session.id, *all)?;
            let rows: Vec<Vec<String>> = children
                .iter()
                .map(|child| {
                    vec![
                        child.name.clone(),
                        child.status.label().to_string(),
                        child.model.clone(),
                        child.usage.total_tokens().to_string(),
                        clip(&child.prompt, 44),
                    ]
                })
                .collect();

            Ok(Output::new(
                if rows.is_empty() {
                    "no subagents".to_string()
                } else {
                    table(&["name", "status", "model", "tokens", "task"], &rows)
                },
                json!({ "subagents": children }),
            ))
        }

        AgentCommand::Settle {
            selector,
            status,
            input_tokens,
            output_tokens,
        } => {
            let status = match status.trim().to_ascii_lowercase().as_str() {
                "completed" | "complete" | "done" => SubagentStatus::Completed,
                "failed" | "error" => SubagentStatus::Failed,
                "cancelled" | "canceled" => SubagentStatus::Cancelled,
                other => {
                    return Err(DomainError::invalid(format!(
                        "unknown status '{other}'; expected completed, failed or cancelled"
                    )))
                }
            };

            let child = app.subagents.settle(
                &session.id,
                selector,
                status,
                Usage {
                    input_tokens: *input_tokens,
                    output_tokens: *output_tokens,
                    turns: 1,
                    attributed_child_tokens: 0,
                },
            )?;

            Ok(Output::new(
                format!(
                    "{} is {} ({} tokens attributed to {})",
                    child.name,
                    child.status.label(),
                    child.usage.total_tokens(),
                    session.name
                ),
                json!({ "child_id": child.child_id, "status": child.status.label() }),
            ))
        }

        AgentCommand::Delete { selector } => {
            let child = app.subagents.delete(&session.id, selector)?;
            Ok(Output::new(
                format!(
                    "{} is no longer addressable; its transcript is kept at {}",
                    child.name, child.session_dir
                ),
                json!({ "child_id": child.child_id }),
            ))
        }
    }
}

fn spawn(app: &App, session: &Session, args: &SpawnArgs) -> Result<Output> {
    let handle = app.subagents.spawn(Spawn {
        parent_selector: session.id.clone(),
        prompt: args.prompt.clone(),
        name: args.name.clone(),
        model: args.model.clone(),
        runner: args.with.clone(),
        system_prompt: args.system_prompt.clone(),
    })?;

    Ok(Output::new(
        format!(
            "admitted {} on {} (depth {})\nit will not reply here; read `pa inbox` or its files",
            handle.name, handle.model, handle.depth
        ),
        json!({
            "child_id": handle.child_id,
            "name": handle.name,
            "session_id": handle.session_id,
            "session_dir": handle.session_dir,
            "model": handle.model,
            "depth": handle.depth,
        }),
    ))
}

pub fn send(app: &App, args: &SendArgs) -> Result<Output> {
    let session = app.session()?;

    // `pa send all "..."` is the broadcast form, so the target doubles as the
    // role when it names one.
    let role = if args.target.eq_ignore_ascii_case("all") {
        ReceiverRole::Broadcast
    } else {
        ReceiverRole::parse(&args.role)?
    };

    let receipts = app.messaging.send(Send {
        from_selector: session.id.clone(),
        role,
        to_name: (!args.target.eq_ignore_ascii_case("all")).then(|| args.target.clone()),
        body: args.message.clone(),
        mode: DeliveryMode::parse(&args.mode)?,
    })?;

    let text = receipts
        .iter()
        .map(|receipt| {
            format!(
                "{} → {}{}",
                receipt.delivery_status.label(),
                receipt.receiver_name,
                receipt
                    .note
                    .as_ref()
                    .map(|note| format!(" ({note})"))
                    .unwrap_or_default()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let failed = receipts
        .iter()
        .filter(|receipt| receipt.delivery_status == DeliveryStatus::Failed)
        .count();

    Ok(Output::new(text, json!({ "receipts": receipts })).with_code(if failed > 0 { exit::NEGATIVE } else { 0 }))
}

pub fn inbox(app: &App, consume: bool) -> Result<Output> {
    let session = app.session()?;
    let messages = if consume {
        app.messaging.consume(&session.id)?
    } else {
        app.messaging.inbox(&session.id)?
    };

    let text = if messages.is_empty() {
        "no messages".to_string()
    } else {
        messages
            .iter()
            .map(|message| format!("from {}: {}", message.sender_name, message.body))
            .collect::<Vec<_>>()
            .join("\n\n")
    };

    Ok(Output::new(
        text,
        json!({ "consumed": consume, "messages": messages }),
    ))
}

pub fn roster(app: &App) -> Result<Output> {
    let session = app.session()?;
    let roster = app.messaging.roster(&session.id)?;

    let rows: Vec<Vec<String>> = roster
        .iter()
        .map(|entry| {
            vec![
                entry.name.clone(),
                entry.role.label().to_string(),
                entry.status.label().to_string(),
            ]
        })
        .collect();

    Ok(Output::new(
        if rows.is_empty() {
            "no one to talk to".to_string()
        } else {
            table(&["name", "role", "status"], &rows)
        },
        json!({
            "roster": roster.iter().map(|entry| json!({
                "name": entry.name,
                "session_id": entry.session_id,
                "role": entry.role.label(),
                "status": entry.status.label(),
            })).collect::<Vec<_>>()
        }),
    ))
}
