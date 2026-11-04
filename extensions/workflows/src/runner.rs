//! The runner: a process of its own (`pi-rust-ext workflows --runner <run dir>`) that runs one
//! workflow to its end. It runs the steps in order, each step's tasks in one agent session, with
//! waits, repeats, goals, approvals, and the context that each agent gets. pi sessions control it
//! through command files and read its `run.json`.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use pi_ext_sdk::CancellationToken;
use pi_ext_sdk::prelude::*;
use tokio::sync::Notify;

use crate::child::{self, Outcome, Status};
use crate::control::{self, Command, Decision};
use crate::launch::{self, Request};
use crate::model::{ContextMode, Mode, Step, Task, fill, format_duration, slug};
use crate::planner;
use crate::policy::{INBOX_ENV, RUN_ENV, STEP_ENV};
use crate::runs::{
    self, Approval, RUN_FILE, RunState, RunStatus, Runs, StepRun, StepStatus, now_ms,
};

pub const FLAG: &str = "--runner";
pub const SCHEMA_FILE: &str = "report.schema.json";
pub const MEMORY_FILE: &str = "memory.md";
pub const LOG_FILE: &str = "runner.log";
const SUMMARY_LIMIT: usize = 4000;
const HISTORY_LIMIT: usize = 12_000;
const MEMORY_LIMIT: usize = 16_000;
const TICK: Duration = Duration::from_millis(250);

/// The signals of the run: stop, skip the wait, and the user's approval.
#[derive(Clone, Default)]
pub struct Control {
    pub cancel: CancellationToken,
    pub skip: Arc<Notify>,
    /// The run stops as interrupted, so it can resume later.
    pub interrupted: Arc<AtomicBool>,
    decision: Arc<Mutex<Option<Decision>>>,
    decided: Arc<Notify>,
}

impl Control {
    fn interrupt(&self) {
        self.interrupted.store(true, Ordering::SeqCst);
        self.cancel.cancel();
    }

    fn decide(&self, decision: Decision) {
        *self.decision.lock().unwrap_or_else(|e| e.into_inner()) = Some(decision);
        self.decided.notify_one();
    }

    fn take_decision(&self) -> Option<Decision> {
        self.decision
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
    }
}

pub struct Ctx {
    pub runs: Runs,
    pub pi: PathBuf,
    pub control: Control,
    pub dir: PathBuf,
}

enum Stop {
    Cancelled,
    Failed(String),
}

/// The runner process: runs the workflow in `dir` while it applies commands, writes the
/// heartbeat, and stops a foreground run when its pi is gone.
pub fn main(dir: Option<&str>) -> ExitCode {
    let Some(dir) = dir.map(PathBuf::from) else {
        pi_ext_sdk::error!("usage: pi-rust-ext workflows {FLAG} <run dir>");
        return ExitCode::from(2);
    };
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            pi_ext_sdk::error!("cannot start the tokio runtime: {err}");
            return ExitCode::FAILURE;
        }
    };
    let code = runtime.block_on(serve(dir));
    runtime.shutdown_background();
    code
}

async fn serve(dir: PathBuf) -> ExitCode {
    let read = std::fs::read_to_string(dir.join(RUN_FILE)).map_err(|e| e.to_string());
    let state =
        match read.and_then(|t| serde_json::from_str::<RunState>(&t).map_err(|e| e.to_string())) {
            Ok(state) => state,
            Err(err) => {
                pi_ext_sdk::error!("cannot read {}: {err}", dir.display());
                return ExitCode::FAILURE;
            }
        };
    ignore_hangup();
    // Commands and messages sent before this runner started belong to an earlier one.
    let _ = control::take(&dir);
    let _ = control::take_steering(&dir.join(control::INBOX_DIR));
    let id = state.id.clone();
    let root = dir
        .parent()
        .and_then(Path::parent)
        .map(Path::to_owned)
        .unwrap_or_default();
    let ctx = Ctx {
        runs: Runs::default(),
        pi: PathBuf::from(&state.pi),
        control: Control::default(),
        dir: dir.clone(),
    };
    ctx.runs.insert(state, dir.clone());
    ctx.runs.update(&id, |run| run.heartbeat_ms = now_ms());
    ctx.runs.persist(&id);
    let (runs, control) = (ctx.runs.clone(), ctx.control.clone());
    let mut engine = tokio::spawn(drive(ctx, id.clone()));
    let mut ticks: u64 = 0;
    loop {
        // The engine writes the final state; nothing writes after it.
        tokio::select! {
            _ = &mut engine => break,
            () = tokio::time::sleep(TICK) => {}
        }
        for command in control::take(&dir) {
            apply(&runs, &id, &control, command);
        }
        ticks += 1;
        if ticks % 4 != 0 {
            continue;
        }
        let orphaned = runs.get(&id).is_some_and(|run| {
            !run.background && run.owner != 0 && !control::alive(&root, run.owner)
        });
        if orphaned {
            control.interrupt();
        }
        runs.update(&id, |run| run.heartbeat_ms = now_ms());
        runs.persist(&id);
    }
    ExitCode::SUCCESS
}

fn apply(runs: &Runs, id: &str, control: &Control, command: Command) {
    match command {
        Command::Stop => control.cancel.cancel(),
        Command::Interrupt => control.interrupt(),
        Command::Skip => control.skip.notify_waiters(),
        Command::Approve { decision } => {
            if runs.get(id).is_some_and(|run| run.approval.is_some()) {
                control.decide(decision);
            }
        }
        Command::Mode { background, owner } => {
            runs.update(id, |run| {
                run.background = background;
                run.owner = owner;
            });
            runs.persist(id);
        }
    }
}

/// A terminal hangup must not end a background run.
fn ignore_hangup() {
    #[cfg(unix)]
    if let Ok(mut hangup) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup()) {
        tokio::spawn(async move { while hangup.recv().await.is_some() {} });
    }
}

/// Runs the workflow to its end and writes the final state; pi sessions report it.
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
        run.end(status);
        run.error = error;
    });
    ctx.runs.persist(&id);
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
    let planned = planner::plan(&ctx.pi, &ctx.dir, &run, index, &ctx.control.cancel).await;
    if ctx.control.cancel.is_cancelled() {
        return Err(Stop::Cancelled);
    }
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
        if !task.confirm || confirm(ctx, id, index, &label).await? {
            run_task(ctx, id, index, task, &label).await?;
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
        catalog: &run.catalog,
        session: &session,
        schema: &ctx.dir.join(SCHEMA_FILE),
        task_skills: &task_skills,
        memory: memory.as_deref(),
        env: vec![
            (RUN_ENV.into(), id.to_owned()),
            (STEP_ENV.into(), step.name.clone()),
            ("PI_TODO_SCOPE".into(), format!("{id}-{}", index + 1)),
            (
                INBOX_ENV.into(),
                ctx.dir.join(control::INBOX_DIR).display().to_string(),
            ),
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
    let output = runs::output_file(&ctx.dir, index);
    let mut written = 0;
    let (outcome, fold) = child::run(&launch, &ctx.control.cancel, |fold| {
        written += runs::append_output(&output, &fold.transcript[written..]);
        ctx.runs.update(id, |run| {
            let s = &mut run.steps[index];
            s.activity = fold.activity.clone();
            s.writing = if fold.writing {
                child::tail(&fold.text, runs::WRITING_TAIL).to_owned()
            } else {
                String::new()
            };
            s.cost = base_cost + fold.cost;
            s.turns = base_turns + fold.turns;
        });
    })
    .await;
    for message in control::take_steering(&ctx.dir.join(control::INBOX_DIR)) {
        let message = crate::ui::one_line(&message, 60);
        log(
            ctx,
            id,
            index,
            format!("not delivered, the agent had finished: {message}"),
        );
    }
    ctx.runs.update(id, |run| {
        let s = &mut run.steps[index];
        s.activity.clear();
        s.writing.clear();
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

/// Waits `duration`. A deadline left by an interrupted run that resumes counts first, so a
/// resumed wait does not start over.
async fn wait(ctx: &Ctx, id: &str, duration: Duration, note: String) -> Result<(), Stop> {
    let left = state(ctx, id)?.wait_left();
    let duration = left.unwrap_or(duration);
    if duration.is_zero() {
        ctx.runs.update(id, |run| run.wait_until_ms = None);
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

/// Waits for the user's approval of a task, from any pi session; `false` skips the task.
async fn confirm(ctx: &Ctx, id: &str, index: usize, label: &str) -> Result<bool, Stop> {
    ctx.control.take_decision();
    ctx.runs.update(id, |run| {
        run.status = RunStatus::Waiting;
        run.note = format!("{label} waits for your approval");
        run.approval = Some(Approval {
            task: label.to_owned(),
            step: index,
            asked_ms: now_ms(),
        });
    });
    ctx.runs.persist(id);
    let decision = loop {
        if let Some(decision) = ctx.control.take_decision() {
            break decision;
        }
        tokio::select! {
            () = ctx.control.cancel.cancelled() => return Err(Stop::Cancelled),
            () = ctx.control.decided.notified() => {}
        }
    };
    ctx.runs.update(id, |run| {
        run.status = RunStatus::Running;
        run.approval = None;
        run.note.clear();
    });
    let word = match decision {
        Decision::Run => "approved",
        Decision::Skip => "skipped",
        Decision::Stop => "stopped",
    };
    log(ctx, id, index, format!("{label}: {word} by the user"));
    ctx.runs.persist(id);
    match decision {
        Decision::Run => Ok(true),
        Decision::Skip => Ok(false),
        Decision::Stop => Err(Stop::Cancelled),
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
            let memory = match memory.trim() {
                "" => "(empty)".to_owned(),
                text => newest(text, MEMORY_LIMIT),
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

/// The end of `text`: the shared notes grow, and a prompt argument has a size limit.
fn newest(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("(older notes are left out)\n…{}", &text[start..])
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
pub fn report_details(run: &RunState) -> Value {
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

pub fn report_text(run: &RunState) -> String {
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
        let long = format!("old fact\n{}\nnewest fact", "x".repeat(MEMORY_LIMIT));
        let clipped = compose(&run, 1, task, 0, Some(&long));
        assert!(clipped.contains("(older notes are left out)") && clipped.contains("newest fact"));
        assert!(!clipped.contains("old fact"), "the oldest notes go first");
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
