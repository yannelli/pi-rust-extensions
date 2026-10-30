use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use crate::app::{App, DraftMode};
use crate::lifecycle::{self, NewGoal};
use crate::tasks::{FlatTask, merge_tasks};
use crate::{draft, widget};

const OTHER: &str = "Other (type an answer)";

#[derive(Debug, Clone, Deserialize)]
pub struct Question {
    #[serde(default)]
    pub id: Option<String>,
    pub question: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub options: Vec<String>,
    #[serde(default)]
    pub recommended: Option<usize>,
    #[serde(default)]
    pub allow_custom: Option<bool>,
}

fn not_drafting(app: &App) -> Option<ToolResult> {
    (!app.lock().drafting())
        .then(|| ToolResult::error("No goal draft is open; the user starts one with /goal."))
}

fn dialogs(ctx: &ToolCtx) -> bool {
    ctx.snapshot().has_ui && ctx.snapshot().mode.as_deref() != Some("rpc")
}

/// The answer to one question; without dialogs, the recommended option.
async fn ask(host: &Host, question: &Question, ui: bool) -> Result<String> {
    let recommended = question
        .recommended
        .and_then(|i| question.options.get(i))
        .cloned();
    if !ui {
        return Ok(
            recommended.unwrap_or_else(|| "(no UI: proceed with your recommended default)".into())
        );
    }
    let title = match &question.context {
        Some(context) => format!("{}\n{context}", question.question),
        None => question.question.clone(),
    };
    let allow_custom = question.allow_custom.unwrap_or(true);
    if question.options.is_empty() {
        return Ok(host
            .input(&title, None)
            .await?
            .unwrap_or_else(|| "(no answer)".into()));
    }
    let mut labels: Vec<String> = question
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| {
            if Some(i) == question.recommended {
                format!("{o} (recommended)")
            } else {
                o.clone()
            }
        })
        .collect();
    if allow_custom {
        labels.push(OTHER.into());
    }
    let Some(choice) = host.select(&title, &labels).await? else {
        return Ok("(dismissed without an answer)".into());
    };
    if choice == OTHER {
        return Ok(host
            .input(&question.question, None)
            .await?
            .unwrap_or_else(|| "(no answer)".into()));
    }
    let index = labels.iter().position(|l| *l == choice).unwrap_or(0);
    Ok(question.options.get(index).cloned().unwrap_or(choice))
}

pub async fn goal_question(app: App, question: Question, ctx: ToolCtx) -> Result<ToolResult> {
    if let Some(error) = not_drafting(&app) {
        return Ok(error);
    }
    let answer = ask(ctx.host(), &question, dialogs(&ctx)).await?;
    Ok(ToolResult::text(format!("User answered: {answer}")).details(json!({ "answer": answer })))
}

#[derive(Deserialize)]
pub struct QuestionnaireArgs {
    questions: Vec<Question>,
}

pub async fn goal_questionnaire(
    app: App,
    args: QuestionnaireArgs,
    ctx: ToolCtx,
) -> Result<ToolResult> {
    if let Some(error) = not_drafting(&app) {
        return Ok(error);
    }
    let mut lines = Vec::new();
    let mut answers = serde_json::Map::new();
    for (i, question) in args.questions.iter().enumerate() {
        let answer = ask(ctx.host(), question, dialogs(&ctx)).await?;
        let id = question.id.clone().unwrap_or_else(|| format!("q{}", i + 1));
        lines.push(format!("- {id} ({}): {answer}", question.question));
        answers.insert(id, Value::String(answer));
    }
    Ok(
        ToolResult::text(format!("Questionnaire answers:\n{}", lines.join("\n")))
            .details(Value::Object(answers)),
    )
}

#[derive(Deserialize)]
pub struct ProposalArgs {
    objective: String,
    #[serde(default)]
    token_budget: Option<Option<u64>>,
    #[serde(default)]
    auto_continue: Option<bool>,
    #[serde(default)]
    sisyphus: Option<bool>,
    #[serde(default)]
    tasks: Vec<FlatTask>,
    #[serde(default)]
    block_completion: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Confirm,
    Continue,
    Cancel,
}

pub fn proposal_lines(args: &ProposalArgs, tasks: &str) -> Vec<String> {
    let mut lines = vec![style::bold("Goal proposal"), String::new()];
    lines.extend(args.objective.lines().map(str::to_owned));
    lines.push(String::new());
    lines.push(style::bold("Tasks"));
    lines.extend(tasks.lines().map(str::to_owned));
    let budget = match args.token_budget {
        Some(Some(b)) => b.to_string(),
        Some(None) => "removed".into(),
        None => "unchanged/none".into(),
    };
    lines.push(String::new());
    lines.push(format!(
        "mode: {} · auto-continue: {} · token budget: {budget}",
        if args.sisyphus.unwrap_or(false) {
            "sisyphus"
        } else {
            "regular"
        },
        args.auto_continue.unwrap_or(true)
    ));
    lines
}

async fn show_proposal(host: &Host, lines: Vec<String>) -> Result<Decision> {
    let footer = style::dim("enter confirm · c/esc continue chatting · x cancel draft · ↑↓ scroll");
    let mut panel = host.panel(lines.clone()).overlay(true).open().await?;
    let mut top = 0usize;
    let height = 30usize;
    loop {
        let width = panel.width().unwrap_or(100);
        let mut view: Vec<String> = lines
            .iter()
            .skip(top)
            .take(height)
            .map(|l| style::truncate(l, width))
            .collect();
        view.push(footer.clone());
        panel.set_lines(view)?;
        let decision = match panel.next_event().await {
            PanelEvent::Key(key) if key.is("enter") || key.char() == Some('y') => Decision::Confirm,
            PanelEvent::Key(key) if key.char() == Some('x') => Decision::Cancel,
            PanelEvent::Key(key) if key.char() == Some('c') || key.is_cancel() => {
                Decision::Continue
            }
            PanelEvent::Key(key) => {
                let max = lines.len().saturating_sub(1);
                top = match () {
                    _ if key.is("up") => top.saturating_sub(1),
                    _ if key.is("down") => (top + 1).min(max),
                    _ if key.is("pageup") => top.saturating_sub(height),
                    _ if key.is("pagedown") => (top + height).min(max),
                    _ => top,
                };
                continue;
            }
            PanelEvent::Resize(_) => continue,
            PanelEvent::Closed => return Ok(Decision::Continue),
        };
        panel.close(Value::Null).await?;
        return Ok(decision);
    }
}

pub async fn propose_goal_draft(app: App, args: ProposalArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host();
    if let Some(error) = not_drafting(&app) {
        return Ok(error);
    }
    if let Err(err) = lifecycle::validate_objective(&app, &args.objective) {
        return Ok(ToolResult::error(err));
    }
    let draft = app.lock().draft.clone().expect("draft checked above");
    let existing = draft
        .target_goal_id
        .as_deref()
        .and_then(|id| {
            app.lock()
                .goals
                .iter()
                .find(|g| g.id == id)
                .map(|g| g.tasks.clone())
        })
        .unwrap_or_default();
    let tasks = match merge_tasks(&args.tasks, &existing) {
        Ok(tasks) => tasks,
        Err(err) => return Ok(ToolResult::error(err)),
    };
    let decision = if dialogs(&ctx) {
        show_proposal(host, proposal_lines(&args, &draft::render_flat(&tasks))).await?
    } else {
        Decision::Confirm
    };
    match decision {
        Decision::Continue => {
            return Ok(ToolResult::text(
                "The user chose Continue Chatting. Ask what to refine, then propose again.",
            ));
        }
        Decision::Cancel => {
            draft::cancel(&app, host).await?;
            return Ok(ToolResult::text(
                "The user cancelled the goal draft. Do not create a goal.",
            ));
        }
        Decision::Confirm => {}
    }
    let goal_id = if draft.mode == DraftMode::Tweak {
        let target = draft.target_goal_id.clone().unwrap_or_default();
        let mut inner = app.lock();
        let goal = inner.goals.iter_mut().find(|g| g.id == target);
        let Some(goal) = goal else {
            return Ok(ToolResult::error(
                "The goal under revision no longer exists.",
            ));
        };
        goal.objective = args.objective.trim().to_owned();
        goal.verification_contract = crate::format::extract_verification_contract(&goal.objective);
        if !args.tasks.is_empty() {
            goal.tasks = tasks;
        }
        if let Some(budget) = args.token_budget {
            goal.token_budget = budget;
        }
        if let Some(block) = args.block_completion {
            goal.block_completion = block;
        }
        goal.log("objective revised by /goal-tweak");
        let goal = goal.clone();
        inner.save(&goal);
        goal.id
    } else {
        let new = NewGoal {
            objective: args.objective.clone(),
            sisyphus: args.sisyphus.unwrap_or(draft.mode == DraftMode::Sisyphus),
            token_budget: args.token_budget.flatten(),
            tasks,
            block_completion: args.block_completion.unwrap_or(false),
            auto_continue: args.auto_continue,
        };
        match lifecycle::create(&app, host, new).await {
            Ok(goal) => goal.id,
            Err(err) => return Ok(ToolResult::error(err)),
        }
    };
    draft::cancel(&app, host).await?;
    app.lock().run.stopped = true;
    widget::refresh(&app, host).await?;
    Ok(ToolResult::text(format!("The user confirmed the goal (goalId={goal_id}). End this turn; the goal continues in the next turn."))
        .details(json!({ "goalId": goal_id })))
}
