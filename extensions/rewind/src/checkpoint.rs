use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::git::FileChange;

pub const ENTRY_TYPE: &str = "pi-checkpoint";

/// Data of a `pi-checkpoint` session entry (upstream v2 shape).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    #[serde(default = "version")]
    pub v: u32,
    #[serde(default = "kind")]
    pub kind: String,
    #[serde(default)]
    pub turn_id: String,
    pub user_entry_id: String,
    pub before_commit: String,
    pub after_commit: String,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub file_count: usize,
    #[serde(default)]
    pub file_changes: Vec<FileChange>,
    #[serde(default)]
    pub created_at: Value,
}

fn version() -> u32 {
    2
}

fn kind() -> String {
    "checkpoint".into()
}

impl Checkpoint {
    pub fn added(&self) -> u64 {
        self.file_changes.iter().map(|change| change.added).sum()
    }

    pub fn removed(&self) -> u64 {
        self.file_changes.iter().map(|change| change.removed).sum()
    }

    pub fn summary(&self) -> String {
        match self.file_changes.as_slice() {
            [] => "no file changes".into(),
            [one] => format!("{} +{} -{}", one.path, one.added, one.removed),
            many => format!(
                "{} files changed +{} -{}",
                many.len(),
                self.added(),
                self.removed()
            ),
        }
    }
}

/// Checkpoints in entry order; a later entry for the same user entry replaces the earlier one.
pub fn checkpoints(entries: &[Value]) -> Vec<Checkpoint> {
    let mut found: Vec<Checkpoint> = Vec::new();
    for entry in entries {
        if entry["type"] != "custom" || entry["customType"] != ENTRY_TYPE {
            continue;
        }
        let Ok(checkpoint) = serde_json::from_value::<Checkpoint>(entry["data"].clone()) else {
            continue;
        };
        if checkpoint.kind != "checkpoint" {
            continue;
        }
        match found
            .iter_mut()
            .find(|known| known.user_entry_id == checkpoint.user_entry_id)
        {
            Some(known) => *known = checkpoint,
            None => found.push(checkpoint),
        }
    }
    found
}

/// Id and text of the last user message.
pub fn latest_user_entry(entries: &[Value]) -> Option<(String, String)> {
    entries.iter().rev().find_map(|entry| {
        let message = &entry["message"];
        let is_user = entry["type"] == "message" && message["role"] == "user";
        let id = entry["id"].as_str().filter(|_| is_user)?;
        Some((id.to_string(), message_text(&message["content"])))
    })
}

pub fn message_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter(|block| block["type"] == "text")
            .filter_map(|block| block["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// First line of the prompt, at most `max` characters.
pub fn short_prompt(prompt: &str, max: usize) -> String {
    let line = prompt.lines().find(|line| !line.trim().is_empty());
    let line = line.unwrap_or("(empty prompt)").trim();
    if line.chars().count() <= max {
        return line.to_string();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

pub fn label(index: usize, checkpoint: &Checkpoint) -> String {
    format!(
        "[{}] {} · {}",
        index + 1,
        short_prompt(&checkpoint.prompt, 60),
        checkpoint.summary()
    )
}

pub fn files_changed_since(checkpoints: &[Checkpoint], index: usize) -> usize {
    checkpoints
        .iter()
        .skip(index)
        .map(|checkpoint| checkpoint.file_changes.len())
        .sum()
}

/// The file state for a tree target: a user entry's pre-turn state, else the branch's last turn.
pub fn tree_target_commit(
    all: &[Checkpoint],
    branch: &[Checkpoint],
    target_id: Option<&str>,
) -> Option<String> {
    if let Some(target) = target_id
        && let Some(turn) = all.iter().find(|cp| cp.user_entry_id == target)
    {
        return Some(turn.before_commit.clone());
    }
    branch.last().map(|cp| cp.after_commit.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(user: &str, before: &str, after: &str, files: usize) -> Value {
        let changes: Vec<Value> = (0..files)
            .map(|i| json!({ "path": format!("f{i}"), "added": 2, "removed": 1 }))
            .collect();
        json!({ "type": "custom", "customType": ENTRY_TYPE, "id": format!("c-{user}"), "data": {
            "v": 2, "kind": "checkpoint", "userEntryId": user, "beforeCommit": before,
            "afterCommit": after, "prompt": format!("prompt {user}\nmore"), "fileCount": files,
            "fileChanges": changes
        }})
    }

    #[test]
    fn collects_and_dedupes_checkpoints() {
        let entries = vec![
            json!({ "type": "message", "id": "u1", "message": { "role": "user", "content": "hi" } }),
            entry("u1", "a", "b", 1),
            json!({ "type": "custom", "customType": "other", "data": {} }),
            entry("u2", "b", "c", 0),
            entry("u1", "a", "d", 2),
        ];
        let found = checkpoints(&entries);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].after_commit, "d");
        assert_eq!(found[0].summary(), "2 files changed +4 -2");
        assert_eq!(found[1].summary(), "no file changes");
        assert_eq!(files_changed_since(&found, 1), 0);
        assert_eq!(files_changed_since(&found, 0), 2);
        assert_eq!(label(1, &found[1]), "[2] prompt u2 · no file changes");
    }

    #[test]
    fn finds_latest_user_entry_text() {
        let entries = vec![
            json!({ "type": "message", "id": "u1", "message": { "role": "user", "content": "one" } }),
            json!({ "type": "message", "id": "u2", "message": { "role": "user",
                "content": [{ "type": "text", "text": "two" }, { "type": "image" }] } }),
            json!({ "type": "message", "id": "a1", "message": { "role": "assistant", "content": [] } }),
        ];
        assert_eq!(
            latest_user_entry(&entries),
            Some(("u2".into(), "two".into()))
        );
        assert_eq!(latest_user_entry(&entries[2..]), None);
    }

    #[test]
    fn shortens_prompts() {
        assert_eq!(short_prompt("\n  fix the bug  \nmore", 60), "fix the bug");
        assert_eq!(short_prompt("abcdef", 4), "abc…");
        assert_eq!(short_prompt("", 4), "(em…");
    }

    #[test]
    fn picks_tree_target_commit() {
        let all = checkpoints(&[entry("u1", "a", "b", 1), entry("u2", "b", "c", 1)]);
        assert_eq!(
            tree_target_commit(&all, &all[..1], Some("u2")).as_deref(),
            Some("b")
        );
        assert_eq!(
            tree_target_commit(&all, &all[..1], Some("x")).as_deref(),
            Some("b")
        );
        assert_eq!(tree_target_commit(&all, &all, None).as_deref(), Some("c"));
        assert_eq!(tree_target_commit(&all, &[], Some("x")), None);
    }
}
