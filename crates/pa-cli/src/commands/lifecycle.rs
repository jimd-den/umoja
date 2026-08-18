//! Sessions, daemon health and the tick.

use pa_domain::prelude::*;
use serde_json::json;

use crate::cli::{AgentsArgs, StartArgs};
use crate::exit;
use crate::output::{ago, clip, table, Output};
use crate::wiring::App;

pub fn start(app: &App, args: &StartArgs) -> Result<Output> {
    let name = args
        .name
        .clone()
        .unwrap_or_else(|| app.default_session_name());

    // Starting a session that already exists is a no-op rather than an error:
    // `pa start` is the first thing anyone types, twice.
    let session = match app.resolve(&name) {
        Ok(existing) => existing,
        Err(DomainError::NotFound { .. }) => app.create(Some(name))?,
        Err(other) => return Err(other),
    };

    Ok(Output::new(
        format!(
            "{} ({}) on {} in {}",
            session.name, session.id, session.runner, session.workdir
        ),
        json!({
            "id": session.id,
            "name": session.name,
            "runner": session.runner,
            "workdir": session.workdir,
            "model": session.model,
            "status": session.status.label(),
        }),
    ))
}

pub fn agents(app: &App, args: &AgentsArgs) -> Result<Output> {
    let now = app.env.now();
    let sessions = app.session_service.list(args.all)?;

    let rows: Vec<Vec<String>> = sessions
        .iter()
        .map(|session| {
            vec![
                session.name.clone(),
                session.status.label().to_string(),
                session.runner.clone(),
                format!("{}", session.usage.total_tokens()),
                ago(session.updated_at, now),
            ]
        })
        .collect();

    let text = if rows.is_empty() {
        "no sessions".to_string()
    } else {
        table(&["name", "status", "runner", "tokens", "updated"], &rows)
    };

    Ok(Output::new(
        text,
        json!({ "sessions": sessions.iter().map(describe).collect::<Vec<_>>() }),
    ))
}

fn describe(session: &Session) -> serde_json::Value {
    json!({
        "id": session.id,
        "name": session.name,
        "kind": format!("{:?}", session.kind).to_lowercase(),
        "status": session.status.label(),
        "runner": session.runner,
        "model": session.model,
        "workdir": session.workdir,
        "depth": session.depth,
        "parent_id": session.parent_id,
        "pid": session.pid,
        "usage": {
            "input_tokens": session.usage.input_tokens,
            "output_tokens": session.usage.output_tokens,
            "total_tokens": session.usage.total_tokens(),
            "own_tokens": session.usage.own_tokens(),
            "turns": session.usage.turns,
        },
        "updated_at": session.updated_at.to_rfc3339(),
    })
}

pub fn rename(app: &App, selector: &str, name: &str) -> Result<Output> {
    let session = app.session_service.rename(selector, name)?;
    Ok(Output::new(
        format!("renamed to {}", session.name),
        json!({ "id": session.id, "name": session.name }),
    ))
}

pub fn stop(app: &App, selector: &str, force: bool) -> Result<Output> {
    let session = app.session_service.stop(selector, force)?;
    // The kernel is a separate process and would otherwise outlive the session
    // that owns it, holding its memory for an hour of idle timeout.
    for language in ["python", "node", "shell"] {
        if let Ok(kernel) = app.kernel(Some(language)) {
            let _ = kernel.shutdown(&session.id);
        }
    }
    Ok(Output::new(
        format!("stopped {}", session.name),
        json!({ "id": session.id, "status": session.status.label() }),
    ))
}

pub fn status(app: &App) -> Result<Output> {
    let now = app.env.now();
    let sessions = app.session_service.list(false)?;
    let heartbeats = app.heartbeats.list(None)?;
    let jobs = app.schedules.list(None, false)?;
    let goals = app.goals.active()?;

    let runner_state = match app.runner.probe() {
        Ok(()) => format!("{} ready", app.runner_name),
        Err(error) => format!("{} unavailable ({error})", app.runner_name),
    };

    let mut lines = vec![
        format!("home     {}", app.paths.root().display()),
        format!("runner   {runner_state}"),
        format!("sessions {} live", sessions.len()),
        format!("goals    {} active", goals.len()),
        format!("beats    {} scheduled", heartbeats.len()),
        format!("jobs     {} pending", jobs.len()),
    ];

    if let Some(next) = jobs.iter().filter_map(|job| job.next_tick).min() {
        lines.push(format!("next job {}", ago(next, now)));
    }
    if let Some(next) = heartbeats
        .iter()
        .filter(|beat| beat.status == HeartbeatStatus::Active)
        .map(|beat| beat.next_fire_at)
        .min()
    {
        lines.push(format!("next beat {}", ago(next, now)));
    }

    Ok(Output::new(
        lines.join("\n"),
        json!({
            "home": app.paths.root().display().to_string(),
            "runner": app.runner_name,
            "runner_ready": app.runner.probe().is_ok(),
            "sessions": sessions.len(),
            "active_goals": goals.len(),
            "heartbeats": heartbeats.len(),
            "pending_jobs": jobs.len(),
        }),
    ))
}

pub fn doctor(app: &App, fix: bool) -> Result<Output> {
    let mut findings: Vec<(String, String)> = Vec::new();

    if let Err(error) = app.runner.probe() {
        findings.push((app.runner_name.clone(), error.to_string()));
    }

    // Reconciling is the fix, so it only runs when asked; otherwise the report
    // would change the state it is describing.
    let repairs = if fix {
        app.session_service.reconcile()?
    } else {
        Vec::new()
    };

    for session in app.session_service.list(true)? {
        if let Some(pid) = session.pid {
            findings.push((
                session.name.clone(),
                format!("claims worker {pid}"),
            ));
        }
    }

    for job in app.schedules.list(None, true)? {
        if let Some(error) = &job.last_error {
            findings.push((job.id.clone(), format!("last tick failed: {error}")));
        }
    }

    let mut lines: Vec<String> = repairs
        .iter()
        .map(|(name, detail)| format!("fixed  {name}: {detail}"))
        .collect();
    lines.extend(
        findings
            .iter()
            .map(|(name, detail)| format!("note   {name}: {detail}")),
    );
    if lines.is_empty() {
        lines.push("nothing to report".into());
    }
    if !fix && !findings.is_empty() {
        lines.push("run with --fix to reconcile".into());
    }

    Ok(Output::new(
        lines.join("\n"),
        json!({
            "repairs": repairs.iter().map(|(name, detail)| json!({"name": name, "detail": detail})).collect::<Vec<_>>(),
            "notes": findings.iter().map(|(name, detail)| json!({"name": name, "detail": detail})).collect::<Vec<_>>(),
        }),
    ))
}

pub fn shutdown(app: &App, force: bool) -> Result<Output> {
    let mut stopped = Vec::new();

    for session in app.session_service.list(true)? {
        for language in ["python", "node", "shell"] {
            if let Ok(kernel) = app.kernel(Some(language)) {
                let _ = kernel.shutdown(&session.id);
            }
        }
        if session.status.is_live()
            && app.session_service.stop(&session.id, force).is_ok()
        {
            stopped.push(session.name);
        }
    }

    Ok(Output::new(
        if stopped.is_empty() {
            "nothing was running".to_string()
        } else {
            format!("stopped {}", stopped.join(", "))
        },
        json!({ "stopped": stopped }),
    ))
}

pub fn log(app: &App, lines: usize, follow: bool) -> Result<Output> {
    let session = app.session()?;

    if follow {
        return follow_transcript(app, &session, lines);
    }

    let records = app.transcript.read(&session.id, Some(lines))?;

    let text = records
        .iter()
        .map(|record| format!("{}  {}", record.at.format("%H:%M:%S"), record.summary()))
        .collect::<Vec<_>>()
        .join("\n");

    Ok(Output::new(
        if text.is_empty() {
            "no events yet".to_string()
        } else {
            text
        },
        json!({ "session": session.name, "events": records }),
    ))
}

/// Reattach to a session.
///
/// # Why this is not a terminal multiplexer
///
/// Prime Agent attaches to a live worker process holding a conversation. This
/// tool has no such process to hold: `pa` exits after every command, work runs
/// detached, and everything that matters — the namespace, the harness, the
/// transcript, the children — is on disk precisely so that no terminal owns
/// it.
///
/// That makes reattaching a *read* rather than a handshake, and it is the
/// better bargain: there is no session to lose when a shell dies, no daemon to
/// resurrect, and a session can be attached from several terminals at once
/// without any of them fighting over a pty.
pub fn attach(app: &App, selector: Option<&str>, lines: usize, follow: bool) -> Result<Output> {
    let session = match selector {
        Some(selector) => app.sessions.resolve(selector)?,
        None => app.session()?,
    };

    let children = app.subagents.list(&session.id, false)?;
    let live = children.iter().filter(|c| c.status.is_live()).count();
    let goal = app.goals.get(&session.id)?;

    let mut header = format!(
        "{} · {} · {} on {}\n{} tokens · {} children ({live} live)",
        session.name,
        session.status.label(),
        session.runner,
        session.model.as_deref().unwrap_or("default"),
        session.usage.total_tokens(),
        children.len(),
    );
    if let Some(goal) = &goal {
        header.push_str(&format!("\ngoal: {}", clip(&goal.objective, 60)));
    }

    if !follow {
        let records = app.transcript.read(&session.id, Some(lines))?;
        let body = records
            .iter()
            .map(|record| format!("{}  {}", record.at.format("%H:%M:%S"), record.summary()))
            .collect::<Vec<_>>()
            .join("\n");
        return Ok(Output::new(
            format!("{header}\n\n{body}"),
            json!({ "session": session, "children": children, "events": records }),
        ));
    }

    println!("{header}\n");
    follow_transcript(app, &session, lines)
}

/// Prints the backlog, then every record as it arrives.
///
/// Polls rather than watching the filesystem: the transcript is append-only
/// and a second of latency on a log nobody is staring at continuously is not
/// worth an inotify dependency and its portability story.
fn follow_transcript(app: &App, session: &Session, backlog: usize) -> Result<Output> {
    let mut seen = 0usize;
    let mut printed_any = false;

    loop {
        let records = app.transcript.read(&session.id, None)?;

        // The first pass shows a bounded backlog; every pass after it shows
        // only what is new.
        let from = if seen == 0 {
            records.len().saturating_sub(backlog)
        } else {
            seen
        };

        for record in records.iter().skip(from) {
            println!(
                "{}  {}",
                record.at.format("%H:%M:%S"),
                record.summary()
            );
            printed_any = true;
        }
        seen = records.len();

        // A session that has stopped will not append anything else, so
        // following it forever would be a hang dressed up as a feature.
        let current = app.sessions.get(&session.id)?;
        if !matches!(current.status, SessionStatus::Running) {
            return Ok(Output::new(
                format!("\n{} is {}", current.name, current.status.label()),
                json!({ "session": current.name, "status": current.status.label() }),
            ));
        }

        let _ = printed_any;
        std::thread::sleep(std::time::Duration::from_millis(700));
    }
}

pub fn tick(app: &App, dry_run: bool) -> Result<Output> {
    if dry_run {
        let now = app.env.now();
        let beats = app.heartbeats.due()?;
        let jobs = app.schedules.due()?;
        let goals = app.goals.active()?;

        let mut lines: Vec<String> = beats
            .iter()
            .map(|beat| format!("heartbeat  {}", clip(&beat.prompt, 60)))
            .collect();
        lines.extend(
            jobs.iter()
                .map(|job| format!("schedule   {} → {}", job.target, clip(&job.prompt, 48))),
        );
        lines.extend(
            goals
                .iter()
                .map(|goal| format!("goal       {}", clip(&goal.objective, 60))),
        );

        return Ok(Output::new(
            if lines.is_empty() {
                "nothing due".to_string()
            } else {
                lines.join("\n")
            },
            json!({
                "dry_run": true,
                "at": now.to_rfc3339(),
                "due_heartbeats": beats.len(),
                "due_jobs": jobs.len(),
                "active_goals": goals.len(),
            }),
        ));
    }

    let report = app.supervisor.tick()?;
    let text = if report.is_quiet() {
        "nothing due".to_string()
    } else {
        report
            .deliveries
            .iter()
            .map(|delivery| {
                format!(
                    "{:<10} {:<16} {} {}",
                    delivery.kind,
                    delivery.session,
                    if delivery.ok { "ok " } else { "err" },
                    clip(delivery.detail.as_deref().unwrap_or(&delivery.prompt), 60)
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    // A tick that delivered nothing is success; a tick where a delivery failed
    // exits non-zero so a cron wrapper notices.
    let code = if report.failures() > 0 {
        exit::NEGATIVE
    } else {
        0
    };

    Ok(Output::new(
        text,
        json!({
            "deliveries": report.deliveries.iter().map(|delivery| json!({
                "kind": delivery.kind,
                "session": delivery.session,
                "ok": delivery.ok,
                "detail": delivery.detail,
            })).collect::<Vec<_>>(),
            "failures": report.failures(),
        }),
    )
    .with_code(code))
}
