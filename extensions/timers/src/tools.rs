use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Local;
use pi_ext_sdk::Subscription;
use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::clock::{DurationArg, format_duration, format_time, now_ms, parse_at};
use crate::config;
use crate::cron::Cron;
use crate::ensure;
use crate::runtime::{Runtime, WAKE_CHANNEL};
use crate::state::{Heartbeat, Schedule, Trigger, new_id};

pub const DEFAULT_HEARTBEAT: &str = "15m";
/// The shortest interval of a repeating schedule.
pub const MIN_SCHEDULE_SECS: u64 = 60;

#[derive(Debug, Deserialize)]
pub struct SleepArgs {
    pub seconds: f64,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct HeartbeatArgs {
    pub action: String,
    #[serde(default)]
    pub every: Option<DurationArg>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub max_pulses: Option<u32>,
    #[serde(default)]
    pub id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ScheduleArgs {
    pub action: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub at: Option<String>,
    #[serde(default, rename = "in")]
    pub after: Option<DurationArg>,
    #[serde(default)]
    pub every: Option<DurationArg>,
    #[serde(default)]
    pub cron: Option<String>,
    #[serde(default)]
    pub max_runs: Option<u32>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
}

fn non_empty(value: Option<String>) -> Option<String> {
    value.map(|v| v.trim().to_owned()).filter(|v| !v.is_empty())
}

fn positive(value: Option<u32>, key: &str) -> Result<Option<u32>, String> {
    match value {
        Some(0) => Err(format!("{key} must be at least 1")),
        other => Ok(other),
    }
}

/// Validates `start` arguments into a new heartbeat that first pulses one interval after `now`.
pub fn new_heartbeat(args: HeartbeatArgs, min_secs: u64, now: i64) -> Result<Heartbeat, String> {
    let reason = non_empty(args.reason).ok_or("reason is required to start a heartbeat")?;
    let every_secs = match &args.every {
        Some(every) => every.seconds()?,
        None => crate::clock::parse_duration(DEFAULT_HEARTBEAT)?,
    };
    if every_secs < min_secs {
        return Err(format!(
            "every must be at least {}",
            format_duration(min_secs)
        ));
    }
    Ok(Heartbeat {
        id: new_id(),
        name: non_empty(args.name),
        reason,
        every_secs,
        max_pulses: positive(args.max_pulses, "max_pulses")?,
        pulses: 0,
        next_at: now + every_secs as i64 * 1000,
    })
}

/// Validates `create` arguments into a new schedule.
pub fn new_schedule(args: ScheduleArgs, now: i64) -> Result<Schedule, String> {
    let prompt = non_empty(args.prompt).ok_or("prompt is required to create a schedule")?;
    let given = [
        args.at.is_some(),
        args.after.is_some(),
        args.every.is_some(),
        args.cron.is_some(),
    ];
    if given.iter().filter(|g| **g).count() != 1 {
        return Err("give exactly one of at, in, every, or cron".into());
    }
    let trigger = if let Some(at) = &args.at {
        let at = parse_at(at, Local::now())?;
        if at <= now {
            return Err(format!("at {} is in the past", format_time(at)));
        }
        Trigger::At { at }
    } else if let Some(after) = &args.after {
        Trigger::At {
            at: now + after.seconds()? as i64 * 1000,
        }
    } else if let Some(every) = &args.every {
        let secs = every.seconds()?;
        if secs < MIN_SCHEDULE_SECS {
            return Err(format!(
                "every must be at least {}",
                format_duration(MIN_SCHEDULE_SECS)
            ));
        }
        Trigger::Every { secs }
    } else {
        let expr = args.cron.unwrap_or_default().trim().to_owned();
        Cron::parse(&expr)?;
        Trigger::Cron { expr }
    };
    let next_at = match &trigger {
        Trigger::At { at } => *at,
        other => other
            .next_after(now)
            .ok_or("the cron expression never matches")?,
    };
    Ok(Schedule {
        id: new_id(),
        name: non_empty(args.name),
        prompt,
        trigger,
        max_runs: positive(args.max_runs, "max_runs")?,
        runs: 0,
        next_at,
    })
}

fn sleep_spec() -> ToolSpec {
    ToolSpec::new(
        "sleep",
        "Wait a fixed number of seconds, then return. Background tasks, subagents, agent messages, scheduled prompts, and steering messages from the user end the sleep early; queued follow-ups do not. Prefer ending the turn when a notification will wake you anyway; use sleep only to wait a fixed time, such as for a rate limit or a service to come up.",
    )
    .label("Sleep")
    .prompt_snippet("Wait a fixed time; ends early on notifications and steering messages")
    .guideline("Use sleep only to wait a fixed time. To wait for a background task, subagent, or message, end the turn instead; its notification wakes you.")
    .parameters(json!({
        "type": "object",
        "properties": {
            "seconds": { "type": "number", "minimum": 1, "description": "Seconds to wait, at least 1 and at most maxSleepSeconds (default 3600)" },
            "reason": { "type": "string", "description": "What the wait is for" }
        },
        "required": ["seconds"]
    }))
}

fn heartbeat_spec() -> ToolSpec {
    ToolSpec::new(
        "heartbeat",
        "Session watchdog. While a heartbeat is active, each interval in which the agent is idle sends a heartbeat message that starts a turn, stating the reason and how to stop it. Pulses are skipped while the agent is working. Start one to keep long, unattended work moving; ack it once the work is finished.",
    )
    .label("Heartbeat")
    .prompt_snippet("Start, list, or ack a session watchdog that wakes the idle agent every interval")
    .guideline("Ack a heartbeat with action \"ack\" and its id as soon as the work it watches is finished.")
    .parameters(json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["start", "ack", "list"] },
            "every": { "type": ["string", "number"], "description": "Interval for start: a duration such as \"30s\", \"15m\", \"1h\", or a number of minutes. Default: 15m." },
            "reason": { "type": "string", "description": "Why the heartbeat runs; required for start and repeated in every pulse" },
            "name": { "type": "string", "description": "Short label" },
            "max_pulses": { "type": "integer", "minimum": 1, "description": "Stop after this many pulses" },
            "id": { "type": "string", "description": "Heartbeat id, for ack" }
        },
        "required": ["action"]
    }))
}

fn schedule_spec() -> ToolSpec {
    ToolSpec::new(
        "schedule",
        "Create, list, or cancel scheduled prompts. A schedule delivers its prompt as a message that starts a turn at a time (at), after a delay (in), repeatedly (every), or on a 5-field cron expression in local time (cron). It is queued when the agent is busy.",
    )
    .label("Schedule")
    .prompt_snippet("Schedule a prompt for a later time, after a delay, every interval, or on a cron expression")
    .guideline("Use schedule for work that must happen at a set time or repeat; give exactly one of at, in, every, or cron.")
    .parameters(json!({
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["create", "list", "cancel"] },
            "prompt": { "type": "string", "description": "The prompt to deliver; required for create" },
            "at": { "type": "string", "description": "Local datetime (\"2026-05-01 09:00\", \"14:30\") or RFC 3339" },
            "in": { "type": ["string", "number"], "description": "Delay such as \"20m\" or \"2h\", or a number of minutes" },
            "every": { "type": ["string", "number"], "description": "Repeat interval such as \"1h\" (at least 1m), or a number of minutes" },
            "cron": { "type": "string", "description": "5-field cron expression in local time, such as \"0 9 * * 1-5\"" },
            "max_runs": { "type": "integer", "minimum": 1, "description": "Stop a repeating schedule after this many runs" },
            "name": { "type": "string", "description": "Short label" },
            "id": { "type": "string", "description": "Schedule id, for cancel" }
        },
        "required": ["action"]
    }))
}

/// Ends with the next `agent:wake` message; never ends without a subscription.
async fn bus_wake(subscription: &mut Option<Subscription>) -> String {
    let Some(subscription) = subscription else {
        return std::future::pending().await;
    };
    match subscription.recv().await {
        Some(data) => {
            let field = |key: &str| data.get(key).and_then(Value::as_str).unwrap_or_default();
            match (field("source"), field("reason")) {
                ("", "") => "agent:wake".into(),
                (source, "") | ("", source) => source.to_owned(),
                (source, reason) => format!("{source} ({reason})"),
            }
        }
        None => std::future::pending().await,
    }
}

async fn sleep(args: SleepArgs, ctx: ToolCtx, runtime: Arc<Runtime>) -> Result<ToolResult> {
    let max = config::load(&ctx.host().agent_dir(), &ctx.host().cwd()).max_sleep_seconds;
    if !args.seconds.is_finite() || args.seconds < 1.0 || args.seconds > max as f64 {
        return Ok(ToolResult::error(format!(
            "seconds must be between 1 and {max}"
        )));
    }
    let mut local = runtime.wake.subscribe();
    let mut bus = ctx.host().subscribe(WAKE_CHANNEL).await.ok();
    let started = Instant::now();
    let woke = tokio::select! {
        () = tokio::time::sleep(Duration::from_secs_f64(args.seconds)) => None,
        () = ctx.cancelled() => Some("cancelled".to_owned()),
        reason = bus_wake(&mut bus) => Some(reason),
        Ok(reason) = local.recv() => Some(reason),
    };
    let slept = started.elapsed().as_secs();
    let text = match &woke {
        None => format!("Slept {}s", args.seconds.round()),
        Some(reason) => format!("Woke after {slept}s: {reason}"),
    };
    let details =
        json!({ "seconds": args.seconds, "slept": slept, "woke": woke, "reason": args.reason });
    Ok(ToolResult::text(text).details(details))
}

fn heartbeat(args: HeartbeatArgs, ctx: &ToolCtx, runtime: &Runtime) -> ToolResult {
    match args.action.as_str() {
        "start" => {
            let config = config::load(&ctx.host().agent_dir(), &ctx.host().cwd());
            match new_heartbeat(args, config.min_heartbeat_seconds, now_ms()) {
                Ok(beat) => {
                    let text = format!(
                        "Started heartbeat {} every {}; first pulse at {} if the agent is idle. When the work is finished, call heartbeat with action \"ack\" and id \"{}\".",
                        beat.label(),
                        format_duration(beat.every_secs),
                        format_time(beat.next_at),
                        beat.id
                    );
                    runtime.update(|t| t.heartbeats.push(beat.clone()));
                    ToolResult::text(text).details(&beat)
                }
                Err(err) => ToolResult::error(err),
            }
        }
        "ack" => {
            let id = args.id.unwrap_or_default();
            match runtime.update(|t| t.ack(id.trim())) {
                Ok(beat) => ToolResult::text(format!(
                    "Stopped heartbeat {} after {} pulse(s).",
                    beat.label(),
                    beat.pulses
                ))
                .details(&beat),
                Err(err) => ToolResult::error(err),
            }
        }
        "list" => {
            let beats = runtime.read(|t| t.heartbeats.clone());
            let text = if beats.is_empty() {
                "No active heartbeats.".to_owned()
            } else {
                let lines: Vec<String> = beats.iter().map(Heartbeat::line).collect();
                lines.join("\n")
            };
            ToolResult::text(text).details(json!({ "heartbeats": beats }))
        }
        other => ToolResult::error(format!("unknown action {other:?}; use start, ack, or list")),
    }
}

fn schedule(args: ScheduleArgs, runtime: &Runtime) -> ToolResult {
    match args.action.as_str() {
        "create" => match new_schedule(args, now_ms()) {
            Ok(schedule) => {
                let text = format!(
                    "Created schedule {} ({}); next run at {}.",
                    schedule.label(),
                    schedule.trigger.describe(),
                    format_time(schedule.next_at)
                );
                runtime.update(|t| t.schedules.push(schedule.clone()));
                ToolResult::text(text).details(&schedule)
            }
            Err(err) => ToolResult::error(err),
        },
        "cancel" => {
            let id = args.id.unwrap_or_default();
            match runtime.update(|t| t.cancel(id.trim())) {
                Ok(schedule) => {
                    ToolResult::text(format!("Cancelled schedule {}.", schedule.label()))
                        .details(&schedule)
                }
                Err(err) => ToolResult::error(err),
            }
        }
        "list" => {
            let schedules = runtime.read(|t| t.schedules.clone());
            let text = if schedules.is_empty() {
                "No active schedules.".to_owned()
            } else {
                let lines: Vec<String> = schedules.iter().map(Schedule::line).collect();
                lines.join("\n")
            };
            ToolResult::text(text).details(json!({ "schedules": schedules }))
        }
        other => ToolResult::error(format!(
            "unknown action {other:?}; use create, list, or cancel"
        )),
    }
}

pub fn register(ext: Extension, runtime: Arc<Runtime>) -> Extension {
    let (r1, r2, r3) = (runtime.clone(), runtime.clone(), runtime);
    ext.tool(sleep_spec(), move |args: SleepArgs, ctx: ToolCtx| {
        let runtime = r1.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &runtime);
            sleep(args, ctx, runtime).await
        }
    })
    .tool(
        heartbeat_spec(),
        move |args: HeartbeatArgs, ctx: ToolCtx| {
            let runtime = r2.clone();
            async move {
                ensure(ctx.host(), ctx.snapshot(), &runtime);
                let result = heartbeat(args, &ctx, &runtime);
                runtime.refresh_status(ctx.host()).await;
                Ok(result)
            }
        },
    )
    .tool(schedule_spec(), move |args: ScheduleArgs, ctx: ToolCtx| {
        let runtime = r3.clone();
        async move {
            ensure(ctx.host(), ctx.snapshot(), &runtime);
            let result = schedule(args, &runtime);
            runtime.refresh_status(ctx.host()).await;
            Ok(result)
        }
    })
}
