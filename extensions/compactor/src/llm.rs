//! Model-written summaries through `llm.complete`, with pi's summarization prompt.

use std::collections::BTreeSet;

use pi_ext_sdk::{Host, LlmRequest};
use serde_json::Value;

use crate::text::{str_of, text_of};

pub const SYSTEM_PROMPT: &str = "You are a context summarization assistant. Your task is to read a conversation between a user and an AI assistant, then produce a structured summary following the exact format specified.\n\nDo NOT continue the conversation. Do NOT respond to any questions in the conversation. ONLY output the structured summary.";
const PROMPT: &str = "The messages above are a conversation to summarize. Create a structured context checkpoint summary that another LLM will use to continue the work.\n\nUse this EXACT format:\n\n## Goal\n[What is the user trying to accomplish? Can be multiple items if the session covers different tasks.]\n\n## Constraints & Preferences\n- [Any constraints, preferences, or requirements mentioned by user]\n- [Or \"(none)\" if none were mentioned]\n\n## Progress\n### Done\n- [x] [Completed tasks/changes]\n\n### In Progress\n- [ ] [Current work]\n\n### Blocked\n- [Issues preventing progress, if any]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale]\n\n## Next Steps\n1. [Ordered list of what should happen next]\n\n## Critical Context\n- [Any data, examples, or references needed to continue]\n- [Or \"(none)\" if not applicable]\n\n";
const UPDATE_PROMPT: &str = "The messages above are NEW conversation messages to incorporate into the existing summary provided in <previous-summary> tags.\n\nUpdate the existing structured summary with new information. RULES:\n- PRESERVE all existing information from the previous summary\n- ADD new progress, decisions, and context from the new messages\n- UPDATE the Progress section: move items from \"In Progress\" to \"Done\" when completed\n- UPDATE \"Next Steps\" based on what was accomplished\n- PRESERVE exact file paths, function names, and error messages\n- If something is no longer relevant, you may remove it\n\nUse this EXACT format:\n\n## Goal\n[Preserve existing goals, add new ones if the task expanded]\n\n## Constraints & Preferences\n- [Preserve existing, add new ones discovered]\n\n## Progress\n### Done\n- [x] [Include previously done items AND newly completed items]\n\n### In Progress\n- [ ] [Current work - update based on progress]\n\n### Blocked\n- [Current blockers - remove if resolved]\n\n## Key Decisions\n- **[Decision]**: [Brief rationale] (preserve all previous, add new)\n\n## Next Steps\n1. [Update based on current state]\n\n## Critical Context\n- [Preserve important context, add new if needed]\n\n";
const FORMAT_TAIL: &str =
    "Keep each section concise. Preserve exact file paths, function names, and error messages.";
const TOOL_RESULT_MAX_CHARS: usize = 2000;

fn text_blocks(content: &Value, separator: &str) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(parts) => parts
            .iter()
            .filter(|p| p["type"] == "text")
            .filter_map(|p| p["text"].as_str())
            .collect::<Vec<_>>()
            .join(separator),
        _ => String::new(),
    }
}

fn truncate(text: &str, max: usize) -> String {
    let total = text.chars().count();
    if total <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}\n\n[... {} more characters truncated]", total - max)
}

fn tool_call_text(call: &Value) -> String {
    let args: Vec<String> = call["arguments"]
        .as_object()
        .map(|args| args.iter().map(|(k, v)| format!("{k}={v}")).collect())
        .unwrap_or_default();
    format!("{}({})", str_of(call, "name"), args.join(", "))
}

/// Labeled transcript of agent messages; custom roles render as user text, thinking is left out.
pub fn serialize_conversation(messages: &[Value]) -> String {
    let mut parts = Vec::new();
    for message in messages {
        let content = &message["content"];
        match str_of(message, "role") {
            "user" => {
                let text = text_blocks(content, "");
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            "assistant" => {
                let blocks = content.as_array().cloned().unwrap_or_default();
                if content.is_string() || blocks.iter().any(|b| b["type"] == "text") {
                    parts.push(format!("[Assistant]: {}", text_blocks(content, "\n")));
                }
                let calls: Vec<String> = blocks
                    .iter()
                    .filter(|b| b["type"] == "toolCall")
                    .map(tool_call_text)
                    .collect();
                if !calls.is_empty() {
                    parts.push(format!("[Assistant tool calls]: {}", calls.join("; ")));
                }
            }
            "toolResult" => {
                let text = text_blocks(content, "");
                if !text.is_empty() {
                    parts.push(format!(
                        "[Tool result]: {}",
                        truncate(&text, TOOL_RESULT_MAX_CHARS)
                    ));
                }
            }
            "bashExecution" => {
                let output = str_of(message, "output");
                let body = if output.is_empty() {
                    "(no output)".to_owned()
                } else {
                    format!("```\n{output}\n```")
                };
                parts.push(format!(
                    "[User]: Ran `{}`\n{body}",
                    str_of(message, "command")
                ));
            }
            "custom" => {
                let text = text_of(content);
                if !text.is_empty() {
                    parts.push(format!("[User]: {text}"));
                }
            }
            "branchSummary" | "compactionSummary" => {
                parts.push(format!(
                    "[User]: <summary>\n{}\n</summary>",
                    str_of(message, "summary")
                ));
            }
            _ => {}
        }
    }
    parts.join("\n\n")
}

/// The full summarization prompt for `messages`.
pub fn summary_prompt(messages: &[Value], previous: Option<&str>, focus: Option<&str>) -> String {
    let mut instructions = String::from(if previous.is_some() {
        UPDATE_PROMPT
    } else {
        PROMPT
    });
    instructions.push_str(FORMAT_TAIL);
    if let Some(focus) = focus.filter(|f| !f.is_empty()) {
        instructions.push_str(&format!("\n\nAdditional focus: {focus}"));
    }
    let mut prompt = format!(
        "<conversation>\n{}\n</conversation>\n\n",
        serialize_conversation(messages)
    );
    if let Some(previous) = previous {
        prompt.push_str(&format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n"
        ));
    }
    prompt.push_str(&instructions);
    prompt
}

/// pi's `<read-files>` and `<modified-files>` tail from `preparation.fileOps`.
pub fn file_ops_tail(file_ops: &Value) -> String {
    let list = |key: &str| -> Vec<String> {
        file_ops[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    };
    let changed: BTreeSet<String> = list("edited").into_iter().chain(list("written")).collect();
    let read: Vec<String> = list("read")
        .into_iter()
        .filter(|p| !changed.contains(p))
        .collect();
    let mut sections = Vec::new();
    if !read.is_empty() {
        sections.push(format!("<read-files>\n{}\n</read-files>", read.join("\n")));
    }
    if !changed.is_empty() {
        let modified: Vec<String> = changed.into_iter().collect();
        sections.push(format!(
            "<modified-files>\n{}\n</modified-files>",
            modified.join("\n")
        ));
    }
    if sections.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", sections.join("\n\n"))
    }
}

/// A model summary of `messages`: `(text, usage)`.
pub async fn summarize(
    host: &Host,
    messages: &[Value],
    previous: Option<&str>,
    focus: Option<&str>,
) -> pi_ext_sdk::Result<(String, Value)> {
    let request =
        LlmRequest::prompt(summary_prompt(messages, previous, focus)).system(SYSTEM_PROMPT);
    let response = host.complete(request).await?;
    if response.text.trim().is_empty() {
        pi_ext_sdk::bail!("Summarization returned no text");
    }
    Ok((response.text, response.message["usage"].clone()))
}

/// Active-work blocks as the `[Active Work]` header of a model summary.
pub fn with_active_work(summary: &str, active: &[(String, String)]) -> String {
    if active.is_empty() {
        return summary.to_owned();
    }
    let blocks: Vec<&str> = active.iter().map(|(_, text)| text.as_str()).collect();
    format!("[Active Work]\n{}\n\n{summary}", blocks.join("\n\n"))
}
