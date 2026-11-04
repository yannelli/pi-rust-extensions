//! Goal tool definitions.

use pi_ext_sdk::prelude::*;

use crate::draft::flat_task_schema;

pub fn goal_tools() -> [ToolSpec; 5] {
    [
        ToolSpec::new("create_goal", "Create and focus a goal on explicit user request.")
            .label("Create goal")
            .parameters(json!({ "type": "object", "properties": {
                "objective": { "type": "string", "description": "Preserve steps and done criteria" },
                "mode": { "type": "string", "enum": ["regular", "sisyphus"], "default": "regular", "description": "sisyphus on user request" },
                "token_budget": { "type": "integer", "minimum": 1, "description": "User-supplied only" }
            }, "required": ["objective"] })),
        ToolSpec::new("get_goal", "Read goal state when the visible snapshot is insufficient.")
            .label("Get goal")
            .parameters(json!({ "type": "object", "properties": {
                "section": { "type": "string", "enum": ["summary", "objective", "tasks", "history"], "default": "summary" },
                "task_id": { "type": "string", "description": "section=tasks: one task" },
                "cursor": { "type": "string", "maxLength": 256, "description": "4000-char pages; repeat section/task" },
                "verbose": { "type": "boolean", "description": "Full legacy output" },
                "include_history": { "type": "boolean" }
            } })),
        ToolSpec::new("update_goal", "Update goal status or save continuation when work stops.")
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
                "reason": { "type": "string", "description": "paused/blocked: required blocker" },
                "attempted_actions": { "type": "array", "items": { "type": "string", "maxLength": 240 }, "maxItems": 8 },
                "suggested_action": { "type": "string", "description": "paused: next step" },
                "completion_summary": { "type": "string", "description": "Claim, not evidence" }
            } })),
        ToolSpec::new("set_goal_tasks", "Replace the task tree with user confirmation, ending the turn.")
            .label("Set goal tasks")
            .parameters(json!({ "type": "object", "properties": {
                "tasks": { "type": "array", "description": "Flat tree; matching IDs retain progress; omitted IDs deleted", "items": flat_task_schema() },
                "block_completion": { "type": "boolean", "default": false, "description": "Gate completion on tasks" },
                "change_summary": { "type": "string" }
            }, "required": ["tasks"] })),
        ToolSpec::new("update_goal_task", "Update task progress during work, singly or in an atomic ordered batch.")
            .label("Update goal task")
            .parameters(json!({ "type": "object", "properties": {
                "task_id": { "type": "string", "description": "With updates, root fields must match an entry" },
                "status": { "type": "string", "enum": ["start", "complete", "skipped", "pending", ""] },
                "updates": { "type": "array", "maxItems": 100, "description": "Empty requires valid root task_id/status", "items": {
                    "type": "object", "additionalProperties": false, "required": ["task_id", "status"], "properties": {
                        "task_id": { "type": "string" }, "status": { "type": "string", "enum": ["start", "complete", "skipped", "pending"] },
                        "evidence": { "type": "string" }, "reason": { "type": "string" } } } },
                "evidence": { "type": "string", "maxLength": 200 },
                "reason": { "type": "string", "description": "Required for skipped" }
            } })),
    ]
}

pub fn draft_tools() -> [ToolSpec; 3] {
    [
        ToolSpec::new("goal_question", "Ask a question during goal drafting.")
            .label("Goal question")
            .parameters(json!({ "type": "object", "required": ["question"], "properties": {
                "question": { "type": "string" },
                "options": { "type": "array", "items": { "type": "string" } },
                "recommended": { "type": "integer", "minimum": 0, "description": "0-based option index" },
                "allow_custom": { "type": "boolean", "default": true }
            } })),
        ToolSpec::new("goal_questionnaire", "Group questions during goal drafting.")
            .label("Goal questionnaire")
            .parameters(json!({ "type": "object", "required": ["questions"], "properties": {
                "questions": { "type": "array", "items": { "type": "object", "required": ["id", "question", "options"], "properties": {
                    "id": { "type": "string" },
                    "question": { "type": "string" },
                    "context": { "type": "string" },
                    "options": { "type": "array", "items": { "type": "string" } },
                    "recommended": { "type": "integer", "minimum": 0 },
                    "allow_custom": { "type": "boolean" }
                } } }
            } })),
        ToolSpec::new("propose_goal_draft", "Request user confirmation when the goal draft is ready.")
            .label("Propose goal draft")
            .parameters(json!({ "type": "object", "required": ["objective"], "properties": {
                "objective": { "type": "string", "description": "Preserve exactly for budget-only tweaks" },
                "token_budget": { "type": ["integer", "null"], "minimum": 1, "description": "User-requested lifetime limit; null removes; omitted retains on tweak" },
                "auto_continue": { "type": "boolean", "default": true },
                "sisyphus": { "type": "boolean", "description": "User-selected mode" },
                "tasks": { "type": "array", "description": "Flat tree", "items": flat_task_schema() },
                "block_completion": { "type": "boolean", "description": "Gate completion on tasks" }
            } })),
    ]
}
