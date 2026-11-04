//! The run loop: steps in order, each step's tasks in one agent session, with waits, repeats,
//! goals, confirmations, and the context that each agent gets.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::child::{self, Outcome, Status};
use crate::launch::{self, Catalog, Request};
use crate::model::{ContextMode, Mode, Step, Task, fill, format_duration, slug};
use crate::planner;
use crate::policy::{RUN_ENV, STEP_ENV};
use crate::runs::{Control, Origin, RunState, RunStatus, Runs, StepRun, StepStatus, now_ms};

pub const SCHEMA_FILE: &str = "report.schema.json";
pub const MEMORY_FILE: &str = "memory.md";
pub const MESSAGE_TYPE: &str = "workflow-run";
const SUMMARY_LIMIT: usize = 4000;
const HISTORY_LIMIT: usize = 12_000;

pub struct Ctx {
    pub host: Host,
    pub runs: Runs,
    pub pi: PathBuf,
    pub catalog: Catalog,
    pub control: Control,
    pub dir: PathBuf,
}

enum Stop {
    Cancelled,
    Failed(String),
}

/// Runs the workflow to its end, then reports to the pi session.
pub async fn drive(ctx: Ctx, id: String) {
    let (status, error) = match steps(&ctx, &id).await {
        Ok(()) => (RunStatus::Done, None),
        Err(Stop::Cancelled) if ctx.control.interrupted.load(Ordering::SeqCst) => {
            (RunStatus::Interrupted, None)
        }
        Err(Stop::Cancelled) => (RunStatus::Cancelled, None),
        Err(Stop::Failed(reason)) => (RunStatus::Failed, Some(reason)),
    };
    ctx.runs.update(&id, |run| {
        run.status = status;
        run.error = error;
        run.ended_ms = Some(now_ms());
        run.wait_until_ms = None;
        run.note.clear();
        for step in &mut run.steps {
            step.activity.clear();
        }
    });
    ctx.runs.set_control(&id, None);
    ctx.runs.persist(&id);
    if let Some(run) = ctx
        .runs
        .get(&id)
        .filter(|r| r.status != RunStatus::Interrupted)
    {
        let message = CustomMessage::new(MESSAGE_TYPE, report_text(&run))
            .display(true)
            .details(report_details(&run))
            .deliver_as(DeliverAs::FollowUp)
            .trigger_turn(run.origin == Origin::Agent);
        if let Err(err) = ctx.host.send_message(message).await {
            pi_ext_sdk::warn!("cannot report workflow run {id}: {}", err.message());
        }
    }
}

fn state(ctx: &Ctx, id: &str) -> Result<RunState, Stop> {
    ctx.runs.get(id).ok_or(Stop::Cancelled)
}

async fn steps(ctx: &Ctx, id: &str) -> Result<(), Stop> {
    let mut index = 0;
    loop {
        let run = state(ctx, id)?;
        let untouched = run
            .steps
            .get(index)
            .is_none_or(|s| s.runs == 0 && s.tasks_done == 0);
        if run.workflow.mode == Mode::Dynamic && untouched {
            replan(ctx, id, index).await?;
        }
        let run = state(ctx, id)?;
        let Some(step) = run.steps.get(index) else {
            return Ok(());
        };
        if step.status != StepStatus::Done {
            if let Err(stop) = run_step(ctx, id, index).await {
                let status = match stop {
                    Stop::Cancelled => StepStatus::Pending,
                    Stop::Failed(_) => StepStatus::Failed,
                };
                ctx.runs.update(id, |run| {
                    run.steps[index].status = status;
                    run.steps[index].ended_ms = Some(now_ms());
                });
                return Err(stop);
            }
            ctx.runs.update(id, |run| {
                run.steps[index].status = StepStatus::Done;
                run.steps[index].ended_ms = Some(now_ms());
            });
            ctx.runs.persist(id);
        }
        index += 1;
    }
}

async fn replan(ctx: &Ctx, id: &str, index: usize) -> Result<(), Stop> {
    ctx.runs
        .update(id, |run| run.note = "Planning the next steps".into());
    let run = state(ctx, id)?;
    let available = planner::available(&ctx.host, &ctx.catalog).await;
    let planned = tokio::select! {
        () = ctx.control.cancel.cancelled() => return Err(Stop::Cancelled),
        planned = planner::plan(&ctx.host, &run, index, available) => planned,
    };
    ctx.runs.update(id, |run| match planned {
        Ok(steps) => {
            run.note = format!("The planner planned {} more step(s)", steps.len());
            run.workflow.steps.truncate(index);
            run.workflow.steps.extend(steps);
            run.steps.truncate(index);
            run.steps
                .resize(run.workflow.steps.len(), StepRun::default());
        }
        Err(err) => {
            pi_ext_sdk::warn!("workflow {id}: planner failed: {err}");
            run.note = format!("The planner failed, so the plan stays: {err}");
        }
    });
    ctx.runs.persist(id);
    Ok(())
}

async fn run_step(ctx: &Ctx, id: &str, index: usize) -> Result<(), Stop> {
    let run = state(ctx, id)?;
    let step = run.workflow.steps[index].clone();
    ctx.runs.update(id, |run| {
        let s = &mut run.steps[index];
        s.status = StepStatus::Running;
        s.started_ms.get_or_insert_with(now_ms);
        s.ended_ms = None;
    });
    ctx.runs.persist(id);
    for task_index in run.steps[index].tasks_done..step.tasks.len() {
        let task = &step.tasks[task_index];
        let label = task_label(&step, task_index);
        if !task.confirm || confirm(ctx, id, &label).await? {
            run_task(ctx, id, index, task, &label).await?;
        } else {
            log(ctx, id, index, format!("{label}: skipped by the user"));
        }
        ctx.runs
            .update(id, |run| run.steps[index].tasks_done = task_index + 1);
        ctx.runs.persist(id);
    }
    Ok(())
}

async fn run_task(ctx: &Ctx, id: &str, index: usize, task: &Task, label: &str) -> Result<(), Stop> {
    wait(ctx, id, task.delay(), format!("{label} starts")).await?;
    let max = task.max_runs();
    for attempt in 0..max {
        if attempt > 0 {
            wait(
                ctx,
                id,
                task.every(),
                format!("{label}: run {} of {max}", attempt + 1),
            )
            .await?;
        }
        let run = state(ctx, id)?;
        let memory = (run.workflow.context_of(&run.workflow.steps[index]) == ContextMode::Memory)
            .then(|| std::fs::read_to_string(ctx.dir.join(MEMORY_FILE)).unwrap_or_default());
        let prompt = compose(&run, index, task, attempt, memory.as_deref());
        let runs_text = if max > 1 {
            format!(" (run {} of {max})", attempt + 1)
        } else {
            String::new()
        };
        match agent(ctx, id, index, &prompt).await {
            Outcome::Cancelled => return Err(Stop::Cancelled),
            Outcome::Failed(reason) => {
                log(
                    ctx,
                    id,
                    index,
                    format!("{label}{runs_text}: failed: {}", first_line(&reason)),
                );
                return give_up(ctx, id, index, task, format!("{label}: {reason}"));
            }
            Outcome::Report { status, summary } => {
                let word = match status {
                    Status::Done => "done",
                    Status::Continue => "continue",
                    Status::Blocked => "blocked",
                };
                log(
                    ctx,
                    id,
                    index,
                    format!("{label}{runs_text}: {word}: {}", first_line(&summary)),
                );
                ctx.runs
                    .update(id, |run| run.steps[index].summary = summary.clone());
                ctx.runs.persist(id);
                match status {
                    Status::Blocked => {
                        return give_up(
                            ctx,
                            id,
                            index,
                            task,
                            format!("{label} is blocked: {summary}"),
                        );
                    }
                    Status::Done if task.until.is_some() => return Ok(()),
                    _ => {}
                }
            }
        }
    }
    match &task.until {
        Some(goal) => give_up(
            ctx,
            id,
            index,
            task,
            format!("{label}: the goal is not met after {max} runs: {goal}"),
        ),
        None => Ok(()),
    }
}

/// An optional task ends; any other task stops the workflow.
fn give_up(ctx: &Ctx, id: &str, index: usize, task: &Task, reason: String) -> Result<(), Stop> {
    if task.optional {
        log(
            ctx,
            id,
            index,
            "The task is optional, so the workflow continues.".into(),
        );
        Ok(())
    } else {
        Err(Stop::Failed(reason))
    }
}

/// One agent run of step `index`.
async fn agent(ctx: &Ctx, id: &str, index: usize, prompt: &str) -> Outcome {
    let Ok(run) = state(ctx, id) else {
        return Outcome::Cancelled;
    };
    let step = &run.workflow.steps[index];
    let context = run.workflow.context_of(step);
    let memory = (context == ContextMode::Memory).then(|| ctx.dir.join(MEMORY_FILE));
    let session = session_file(&ctx.dir, index, step, context);
    let task_skills: Vec<String> = step.tasks.iter().filter_map(|t| t.skill.clone()).collect();
    let request = Request {
        scope: &run.workflow.scope(step),
        parent_model: run.parent_model.as_deref(),
        parent_thinking: run.parent_thinking.as_deref(),
        catalog: &ctx.catalog,
        session: &session,
        schema: &ctx.dir.join(SCHEMA_FILE),
        task_skills: &task_skills,
        memory: memory.as_deref(),
        env: vec![
            (RUN_ENV.into(), id.to_owned()),
            (STEP_ENV.into(), step.name.clone()),
            ("PI_TODO_SCOPE".into(), format!("{id}-{}", index + 1)),
        ],
        prompt,
    };
    let launch = match launch::build(&ctx.pi, Path::new(&run.cwd), request) {
        Ok(launch) => launch,
        Err(err) => return Outcome::Failed(err),
    };
    let (base_cost, base_turns) = (run.steps[index].cost, run.steps[index].turns);
    ctx.runs.update(id, |run| {
        let s = &mut run.steps[index];
        s.runs += 1;
        s.model = launch.model.clone();
        s.activity = "starting".into();
    });
    let (outcome, fold) = child::run(&launch, &ctx.control.cancel, |fold| {
        ctx.runs.update(id, |run| {
            let s = &mut run.steps[index];
            s.activity = fold.activity.clone();
            s.cost = base_cost + fold.cost;
            s.turns = base_turns + fold.turns;
        });
    })
    .await;
    ctx.runs.update(id, |run| {
        let s = &mut run.steps[index];
        s.activity.clear();
        s.cost = base_cost + fold.cost;
        s.turns = base_turns + fold.turns;
        if fold.model.is_some() {
            s.model = fold.model.clone();
        }
    });
    outcome
}

/// Each step has its own session file; `session` context shares one.
pub fn session_file(dir: &Path, index: usize, step: &Step, context: ContextMode) -> PathBuf {
    match context {
        ContextMode::Session => dir.join("session.jsonl"),
        _ => dir.join(format!("{:02}-{}.jsonl", index + 1, slug(&step.name))),
    }
}

async fn wait(ctx: &Ctx, id: &str, duration: Duration, note: String) -> Result<(), Stop> {
    if duration.is_zero() {
        return Ok(());
    }
    ctx.runs.update(id, |run| {
        run.status = RunStatus::Waiting;
        run.wait_until_ms = Some(now_ms() + duration.as_millis() as u64);
        run.note = note;
    });
    ctx.runs.persist(id);
    tokio::select! {
        () = ctx.control.cancel.cancelled() => return Err(Stop::Cancelled),
        () = tokio::time::sleep(duration) => {}
        () = ctx.control.skip.notified() => {}
    }
    ctx.runs.update(id, |run| {
        run.status = RunStatus::Running;
        run.wait_until_ms = None;
        run.note.clear();
    });
    Ok(())
}

const RUN: &str = "Run it";
const SKIP: &str = "Skip this task";
const STOP: &str = "Stop the workflow";

/// Asks the user to approve a task; `false` skips it.
async fn confirm(ctx: &Ctx, id: &str, label: &str) -> Result<bool, Stop> {
    if !ctx.host.has_ui() {
        return Err(Stop::Failed(format!(
            "{label} needs the user's approval, and pi has no UI"
        )));
    }
    let name = state(ctx, id)?.workflow.name;
    ctx.runs.update(id, |run| {
        run.status = RunStatus::Waiting;
        run.note = format!("{label} waits for your approval");
    });
    ctx.runs.persist(id);
    let title = format!("Workflow {name}: run \"{label}\"?");
    let choice = tokio::select! {
        () = ctx.control.cancel.cancelled() => return Err(Stop::Cancelled),
        choice = ctx.host.select(&title, &[RUN, SKIP, STOP]) => choice,
    };
    ctx.runs.update(id, |run| {
        run.status = RunStatus::Running;
        run.note.clear();
    });
    match choice.ok().flatten().as_deref() {
        Some(RUN) => Ok(true),
        Some(SKIP) => Ok(false),
        _ => Err(Stop::Cancelled),
    }
}

fn log(ctx: &Ctx, id: &str, index: usize, line: String) {
    ctx.runs.update(id, |run| run.steps[index].log.push(line));
}

pub fn task_label(step: &Step, index: usize) -> String {
    if step.tasks.len() == 1 {
        step.name.clone()
    } else {
        format!("{} {}/{}", step.name, index + 1, step.tasks.len())
    }
}

/// The prompt of one agent run. The first run of a step gets the workflow header and the
/// context; later runs continue the same session.
pub fn compose(
    run: &RunState,
    index: usize,
    task: &Task,
    attempt: u32,
    memory: Option<&str>,
) -> String {
    let workflow = &run.workflow;
    let step = &workflow.steps[index];
    let context = workflow.context_of(step);
    let mut out = String::new();
    if run.steps[index].runs == 0 {
        out.push_str(&format!(
            "# Workflow {}: step {} of {}, \"{}\"\n",
            workflow.name,
            index + 1,
            workflow.steps.len(),
            step.name
        ));
        if !workflow.description.is_empty() {
            out.push_str(&format!("{}\n", workflow.description));
        }
        if let Some(goal) = run
            .inputs
            .get("goal")
            .filter(|_| workflow.mode == Mode::Dynamic)
        {
            out.push_str(&format!("Overall goal: {goal}\n"));
        }
        if matches!(context, ContextMode::Handoff | ContextMode::Memory) {
            out.push_str(&history(run, index));
        }
        if let Some(memory) = memory {
            let memory = if memory.trim().is_empty() {
                "(empty)"
            } else {
                memory.trim()
            };
            out.push_str(&format!(
                "\n## Shared memory\n{memory}\n\nAdd facts that later steps need with the `workflow_memory` tool.\n"
            ));
        }
        out.push_str("\n## Your task\n");
    } else if attempt > 0 {
        let what = if task.until.is_some() {
            "Check again"
        } else {
            "Do the task again with fresh results"
        };
        out.push_str(&format!(
            "Run {} of {}. {what}.\n\n",
            attempt + 1,
            task.max_runs()
        ));
    }
    out.push_str(&fill(&task.prompt, &run.inputs));
    if let Some(goal) = &task.until {
        out.push_str(&format!(
            "\n\nGoal: {}\nReport `done` only when the goal is met. Report `continue` when it is not met yet; \
the workflow then waits {} and runs this task again.",
            fill(goal, &run.inputs),
            format_duration(task.every())
        ));
    }
    match &task.skill {
        Some(skill) => format!("/skill:{skill} {out}"),
        None => out,
    }
}

/// The reports of the steps before `index`, newest kept when they are long.
fn history(run: &RunState, index: usize) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (i, (step, state)) in run
        .workflow
        .steps
        .iter()
        .zip(&run.steps)
        .enumerate()
        .take(index)
    {
        if state.summary.trim().is_empty() {
            continue;
        }
        let status = match state.status {
            StepStatus::Done => "done",
            StepStatus::Failed => "failed",
            _ => "partly done",
        };
        parts.push(format!(
            "### {}. {} ({status})\n{}\n",
            i + 1,
            step.name,
            clip(state.summary.trim(), SUMMARY_LIMIT)
        ));
    }
    let mut total = 0;
    let kept: Vec<String> = parts
        .into_iter()
        .rev()
        .take_while(|part| {
            total += part.len();
            total <= HISTORY_LIMIT
        })
        .collect();
    if kept.is_empty() {
        return String::new();
    }
    let body: Vec<String> = kept.into_iter().rev().collect();
    format!("\n## Reports of earlier steps\n{}", body.join("\n"))
}

fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

pub fn first_line(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    if line.chars().count() > 100 {
        line.chars().take(99).chain(['…']).collect()
    } else {
        line.to_owned()
    }
}

/// What the `workflow-run` message renderer draws.
fn report_details(run: &RunState) -> Value {
    let steps: Vec<Value> = run
        .workflow
        .steps
        .iter()
        .zip(&run.steps)
        .map(|(step, s)| json!({ "name": step.name, "status": s.status, "summary": first_line(&s.summary) }))
        .collect();
    json!({ "id": run.id, "workflow": run.workflow.name, "status": run.status, "stats": run.stats(),
        "steps": steps, "error": run.error })
}

fn report_text(run: &RunState) -> String {
    let mut lines = vec![format!(
        "Workflow **{}** {} ({}). Run id: `{}`.",
        run.workflow.name,
        match run.status {
            RunStatus::Done => "finished",
            RunStatus::Failed => "failed",
            RunStatus::Cancelled => "stopped",
            _ => run.status.label(),
        },
        run.stats(),
        run.id
    )];
    for (step, state) in run.workflow.steps.iter().zip(&run.steps) {
        let glyph = match state.status {
            StepStatus::Done => "✓",
            StepStatus::Failed => "✗",
            _ => "○",
        };
        let summary = first_line(&state.summary);
        lines.push(if summary.is_empty() {
            format!("{glyph} {}", step.name)
        } else {
            format!("{glyph} {}: {summary}", step.name)
        });
    }
    if let Some(error) = &run.error {
        lines.push(format!("Error: {error}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;
    use crate::model::Workflow;

    fn run(context: &str) -> RunState {
        let workflow: Workflow = serde_json::from_value(json!({
            "name": "ship", "description": "Ship it.", "context": context,
            "steps": [
                { "name": "build", "tasks": [{ "prompt": "Build {{thing}}" }] },
                { "name": "ci", "tasks": [{ "prompt": "Watch CI", "until": "CI is green", "every": "2m" }, { "prompt": "Merge", "skill": "merge" }] }
            ]
        }))
        .unwrap();
        RunState {
            steps: vec![
                StepRun {
                    status: StepStatus::Done,
                    summary: "Built the thing.\nMore.".into(),
                    ..StepRun::default()
                },
                StepRun::default(),
            ],
            inputs: BTreeMap::from([("thing".into(), "X".into())]),
            workflow,
            ..RunState::default()
        }
    }

    #[test]
    fn first_run_gets_header_context_and_goal() {
        let run = run("handoff");
        let task = &run.workflow.steps[1].tasks[0];
        let prompt = compose(&run, 1, task, 0, None);
        assert!(
            prompt.starts_with("# Workflow ship: step 2 of 2, \"ci\"\nShip it.\n"),
            "{prompt}"
        );
        assert!(
            prompt.contains("### 1. build (done)\nBuilt the thing."),
            "{prompt}"
        );
        assert!(
            prompt.contains("## Your task\nWatch CI\n\nGoal: CI is green"),
            "{prompt}"
        );
        assert!(prompt.contains("waits 2m"), "{prompt}");
        let first = compose(&run, 0, &run.workflow.steps[0].tasks[0], 0, None);
        assert!(first.ends_with("## Your task\nBuild X"), "{first}");
    }

    #[test]
    fn later_runs_continue_the_session() {
        let mut run = run("memory");
        run.steps[1].runs = 2;
        let task = &run.workflow.steps[1].tasks[0];
        let prompt = compose(&run, 1, task, 1, Some("note"));
        assert!(
            prompt.starts_with("Run 2 of 10. Check again.\n\nWatch CI"),
            "{prompt}"
        );
        let skill = compose(&run, 1, &run.workflow.steps[1].tasks[1], 0, None);
        assert_eq!(skill, "/skill:merge Merge");
        run.steps[1].runs = 0;
        let fresh = compose(&run, 1, task, 0, Some(""));
        assert!(fresh.contains("## Shared memory\n(empty)"), "{fresh}");
        let none = compose(&self::run("none"), 1, task, 0, None);
        assert!(!none.contains("Reports of earlier steps"), "{none}");
    }

    #[test]
    fn sessions_follow_the_context() {
        let step = Step {
            name: "Fix Bugs".into(),
            ..Step::default()
        };
        let dir = Path::new("/r");
        assert_eq!(
            session_file(dir, 2, &step, ContextMode::Handoff),
            Path::new("/r/03-fix-bugs.jsonl")
        );
        assert_eq!(
            session_file(dir, 2, &step, ContextMode::Session),
            Path::new("/r/session.jsonl")
        );
    }
}
