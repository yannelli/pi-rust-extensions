use pi_ext_sdk::prelude::*;

use crate::app::{App, Draft, DraftMode};
use crate::prompts::escape as escape_payload;
use crate::state::{GoalTask, now_ms};
use crate::widget;

pub fn flat_task_schema() -> Value {
    json!({ "type": "object", "required": ["id", "title"], "properties": {
        "id": { "type": "string", "description": "Short stable slug e.g. 'task-1'" },
        "title": { "type": "string", "description": "Human-readable task title" },
        "parent_id": { "type": "string", "description": "Parent id; omit for roots." },
        "verification_contract": { "type": "string", "description": "Required completion evidence." },
        "lightweight_subtasks": { "type": "boolean", "description": "Children do not gate parent completion." }
    } })
}

pub fn render_flat(tasks: &[GoalTask]) -> String {
    let goal = crate::state::Goal {
        tasks: tasks.to_vec(),
        ..crate::state::Goal::new("preview", false, None)
    };
    let lines: Vec<String> = goal
        .ordered_tasks()
        .into_iter()
        .map(|(depth, task)| format!("{}- {}: {}", "  ".repeat(depth), task.id, task.title))
        .collect();
    if lines.is_empty() {
        "(no tasks)".into()
    } else {
        lines.join("\n")
    }
}

/// The user message that starts a guided draft, ported from upstream `goalDraftingPrompt`.
pub fn drafting_prompt(topic: &str, mode: DraftMode, current_objective: Option<&str>) -> String {
    let topic = topic.trim();
    let safe_topic = if topic.is_empty() {
        "(no topic provided — ask the user what they want to accomplish)".to_owned()
    } else {
        escape_payload(topic)
    };
    let header = match mode {
        DraftMode::Sisyphus => {
            "[GOAL CONFIRMATION focus=sisyphus]\nThe user invoked Sisyphus intent discussion (/sisyphus). Help turn their request into a confirmed goal contract. Do NOT start substantive work yet."
        }
        DraftMode::Goal => {
            "[GOAL CONFIRMATION focus=goal]\nThe user invoked guided goal drafting (/goal). Help turn their request into a confirmed goal contract. Do NOT start substantive work yet."
        }
        DraftMode::Tweak => {
            "[GOAL CONFIRMATION focus=tweak]\nThe user invoked /goal-tweak to revise the focused goal. Propose the revised objective with propose_goal_draft. Usage and matching task progress are kept. Do NOT start substantive work yet."
        }
    };
    let mut lines = vec![header.to_owned(), String::new()];
    if let Some(objective) = current_objective {
        lines.push(format!(
            "Current objective:\n<current_objective>\n{}\n</current_objective>\n",
            escape_payload(objective)
        ));
    }
    lines.extend([
        format!("Topic the user provided:\n<goal_topic>\n{safe_topic}\n</goal_topic>"),
        String::new(),
        "Confirmation protocol:".into(),
        "- Treat this as a lightweight conversation with the user, not a separate long-running runtime phase.".into(),
        "- If the topic is vague, ask one focused question with a recommended default. Use goal_question or goal_questionnaire when a structured answer would help, but plain conversation is acceptable.".into(),
        "- Targeted read-only research is allowed when it helps define a better goal contract; do not start implementation before confirmation.".into(),
        "- If the topic is already concrete, you may proceed directly to propose_goal_draft.".into(),
        "- The goal contract should make the objective, success criteria, boundaries, constraints, and blocker rule explicit.".into(),
        "- If the objective naturally decomposes into trackable milestones, include the task list in the `tasks` parameter of `propose_goal_draft` so the user accepts goal and tasks in one confirmation.".into(),
        "- propose_goal_draft opens the user's Confirm / Continue Chatting dialog. Confirm creates and focuses the goal; Continue Chatting means keep refining.".into(),
        "- create_goal is not a shortcut. Direct create_goal calls are rejected so the user keeps explicit say in goal creation.".into(),
        "- The user can stop drafting with /goal-cancel.".into(),
        String::new(),
    ]);
    let shape: &[&str] = match mode {
        DraftMode::Sisyphus => &[
            "Propose a Sisyphus goal in this shape when ready:",
            "=== Sisyphus Goal ===",
            "Objective: <one-sentence outcome>",
            "Success criteria: <observable evidence the whole ordered goal is done>",
            "Boundaries: <in scope / out of scope>",
            "Constraints: <hard rules, files not to touch, etc.>",
            "Verification contract: <optional — what verification evidence is required before marking complete>",
            "Ordered steps: <preserve the user's requested steps and ordering; do not add preflight or reconnaissance steps they did not ask for>",
            "If blocked / unclear / failing: <default = stop and ask the user>",
            "Call propose_goal_draft with sisyphus=true and auto_continue=true unless the user asked otherwise.",
        ],
        _ => &[
            "Propose a normal goal in this shape when ready:",
            "=== Goal ===",
            "Objective: <one-sentence outcome>",
            "Success criteria: <observable evidence the goal is done>",
            "Boundaries: <in scope / out of scope>",
            "Constraints: <hard rules>",
            "Verification contract: <optional — what verification evidence is required before marking complete>",
            "If blocked: <default = stop and ask the user>",
            "Call propose_goal_draft with sisyphus=false and auto_continue=true unless the user asked otherwise.",
        ],
    };
    lines.extend(shape.iter().map(|line| (*line).to_owned()));
    lines.join("\n")
}

/// Short system prompt section while a draft is open.
pub fn draft_section(draft: &Draft) -> String {
    let mode = match draft.mode {
        DraftMode::Goal => "goal",
        DraftMode::Sisyphus => "sisyphus",
        DraftMode::Tweak => "tweak",
    };
    format!(
        "[PI GOAL DRAFT mode={mode}]\nA goal draft is open. Clarify with goal_question or goal_questionnaire, then call propose_goal_draft. Do not start substantive work before the user confirms."
    )
}

/// Opens a draft and sends the drafting prompt as a user message.
pub async fn start(app: &App, host: &Host, mode: DraftMode, topic: &str) -> Result<()> {
    let focused = app
        .lock()
        .focused()
        .map(|g| (g.id.clone(), g.objective.clone()));
    let (target, current) = match (mode, focused) {
        (DraftMode::Tweak, None) => {
            return host.notify_warning("goal: no focused goal to tweak").await;
        }
        (DraftMode::Tweak, Some((id, objective))) => (Some(id), Some(objective)),
        _ => (None, None),
    };
    let draft = Draft {
        mode,
        seed: topic.to_owned(),
        target_goal_id: target,
        started_at: now_ms(),
        cleared_at: None,
    };
    app.set_draft(host, Some(draft)).await?;
    app.install_profile(host).await?;
    widget::refresh(app, host).await?;
    let deliver = if host.is_idle().await.unwrap_or(true) {
        DeliverAs::FollowUp
    } else {
        DeliverAs::Steer
    };
    host.send_user_message(
        drafting_prompt(topic, mode, current.as_deref()),
        Some(deliver),
    )
    .await
}

pub async fn cancel(app: &App, host: &Host) -> Result<bool> {
    if !app.lock().drafting() {
        return Ok(false);
    }
    app.set_draft(host, None).await?;
    app.install_profile(host).await?;
    widget::refresh(app, host).await?;
    Ok(true)
}
