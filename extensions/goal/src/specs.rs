//! Tool definitions with the upstream pi-goal-x names, descriptions, and parameter schemas.

use pi_ext_sdk::prelude::*;

use crate::draft::flat_task_schema;

pub fn goal_tools() -> [ToolSpec; 5] {
    [
        ToolSpec::new("create_goal", "Create and focus a persistent goal only on explicit user request; other open goals remain unchanged.")
            .label("Create goal")
            .parameters(json!({ "type": "object", "properties": {
                "objective": { "type": "string", "description": "Full objective; preserve ordered steps and done criteria." },
                "mode": { "type": "string", "enum": ["regular", "sisyphus"], "description": "Default regular; sisyphus only if requested." },
                "token_budget": { "type": "integer", "minimum": 1, "description": "Whole-token budget, only if supplied by the user." }
            }, "required": ["objective"] })),
        ToolSpec::new("get_goal", "Read focused goal state. Default: compact summary. Retrieve full objective/contracts, tasks, or history in 4000-character pages with section and cursor. verbose preserves full legacy output.")
            .label("Get goal")
            .parameters(json!({ "type": "object", "properties": {
                "section": { "type": "string", "enum": ["summary", "objective", "tasks", "history"] },
                "task_id": { "type": "string", "description": "With section=tasks, retrieve one task." },
                "cursor": { "type": "string", "maxLength": 256, "description": "Next page; repeat section/task." },
                "verbose": { "type": "boolean", "description": "Full detail mode." },
                "include_history": { "type": "boolean" }
            } })),
        ToolSpec::new("update_goal", "Update lifecycle status or optionally save continuation. New waits require strictExecutionContract opt-in. Wait: future ISO deadline; reuse wait_id/deadline without polling on recheck. Completion is audited.")
            .label("Update goal")
            .parameters(json!({ "type": "object", "properties": {
                "status": { "type": "string", "enum": ["complete", "blocked", "paused"], "description": "Exclusive with continuation." },
                "continuation": { "anyOf": [
                    { "type": "object", "properties": { "kind": { "const": "ready" } }, "required": ["kind"], "additionalProperties": false },
                    { "type": "object", "properties": {
                        "kind": { "const": "wait" }, "reason": { "type": "string", "minLength": 1, "maxLength": 2000 },
                        "deadline": { "type": "string" }, "wait_id": { "type": "string" },
                        "polling": { "type": "object", "properties": { "interval_seconds": { "type": "integer", "minimum": 1 }, "max_checks": { "type": "integer", "minimum": 1 } }, "additionalProperties": false }
                    }, "required": ["kind", "reason", "deadline"], "additionalProperties": false }
                ] },
                "reason": { "type": "string", "description": "Required when status is paused or blocked: describe the concrete blocker." },
                "attempted_actions": { "type": "array", "items": { "type": "string", "maxLength": 240 }, "maxItems": 8, "description": "Actions attempted against the blocker." },
                "suggested_action": { "type": "string", "description": "Optional suggested next step when status is paused." },
                "completion_summary": { "type": "string", "description": "Untrusted completion claim; never evidence." }
            } })),
        ToolSpec::new("set_goal_tasks", "Set or restructure the task tree with user confirmation. Matching IDs retain progress; removed IDs are deleted. Confirmation stops the turn; continuation follows.")
            .label("Set goal tasks")
            .parameters(json!({ "type": "object", "properties": {
                "tasks": { "type": "array", "description": "Flat parent-linked task list", "items": flat_task_schema() },
                "block_completion": { "type": "boolean", "description": "Require all tasks resolved; default false." },
                "change_summary": { "type": "string", "description": "Optional summary of the task list change" }
            }, "required": ["tasks"] })),
        ToolSpec::new("update_goal_task", "Update task progress without stopping the turn. Use ordered updates for an atomic batch, or task_id/status for one task. An invalid update rejects the whole batch.")
            .label("Update goal task")
            .parameters(json!({ "type": "object", "properties": {
                "task_id": { "type": "string", "description": "Single-task form; with updates, any non-empty root fields must repeat one batch entry." },
                "status": { "type": "string", "enum": ["start", "complete", "skipped", "pending", ""] },
                "updates": { "type": "array", "maxItems": 100, "description": "Ordered atomic batch. An empty array is allowed only with valid single-task fields.", "items": {
                    "type": "object", "additionalProperties": false, "required": ["task_id", "status"], "properties": {
                        "task_id": { "type": "string" }, "status": { "type": "string", "enum": ["start", "complete", "skipped", "pending"] },
                        "evidence": { "type": "string" }, "reason": { "type": "string" } } } },
                "evidence": { "type": "string", "description": "Completion evidence; max 200 chars." },
                "reason": { "type": "string", "description": "Required for skipped." }
            } })),
    ]
}

pub fn draft_tools() -> [ToolSpec; 3] {
    [
        ToolSpec::new("goal_question", "Ask one structured question during a user-started goal draft.")
            .label("Goal question")
            .parameters(json!({ "type": "object", "required": ["question"], "properties": {
                "question": { "type": "string", "description": "The question to ask." },
                "options": { "type": "array", "items": { "type": "string", "description": "A concise answer option." } },
                "recommended": { "type": "integer", "minimum": 0, "description": "Zero-based recommended option." },
                "allow_custom": { "type": "boolean", "description": "Allow a custom answer; defaults to true." }
            } })),
        ToolSpec::new("goal_questionnaire", "Ask a short structured questionnaire during a user-started goal draft.")
            .label("Goal questionnaire")
            .parameters(json!({ "type": "object", "required": ["questions"], "properties": {
                "questions": { "type": "array", "items": { "type": "object", "required": ["id", "question", "options"], "properties": {
                    "id": { "type": "string", "description": "Stable question id." },
                    "question": { "type": "string", "description": "Question for the user." },
                    "context": { "type": "string", "description": "Optional short context." },
                    "options": { "type": "array", "items": { "type": "string", "description": "Answer option." } },
                    "recommended": { "type": "integer", "minimum": 0 },
                    "allow_custom": { "type": "boolean" }
                } } }
            } })),
        ToolSpec::new("propose_goal_draft", "Present the drafted objective and agent-selected task plan for explicit user confirmation.")
            .label("Propose goal draft")
            .parameters(json!({ "type": "object", "required": ["objective"], "properties": {
                "objective": { "type": "string", "description": "Complete proposed objective; preserve it exactly for budget-only tweaks." },
                "token_budget": { "type": ["integer", "null"], "minimum": 1, "description": "Only on user request: positive whole-token lifetime limit; null removes it. Omit to retain the current tweak budget or create without one." },
                "auto_continue": { "type": "boolean", "description": "Defaults to true." },
                "sisyphus": { "type": "boolean", "description": "Must match the user-selected goal mode." },
                "tasks": { "type": "array", "description": "Flat parent-linked task tree to confirm with the goal.", "items": flat_task_schema() },
                "block_completion": { "type": "boolean", "description": "Require task completion before goal completion." }
            } })),
    ]
}
