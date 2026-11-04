//! `agent_message`: messages between the parent session and its subagent runs through file
//! mailboxes under `<agentDir>/ext/subagents/mailbox/<address>/`. The parent's address is its
//! session id; a run's address is its run id (`PI_SUBAGENT_RUN_ID`), and a run learns its
//! parent's address from `PI_SUBAGENT_PARENT`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::app::App;
use crate::runs::Status;

pub const TOOL: &str = "agent_message";
pub const MESSAGE_TYPE: &str = "agent-message";
pub const PARENT_ENV: &str = "PI_SUBAGENT_PARENT";
pub const RUN_ENV: &str = "PI_SUBAGENT_RUN_ID";
pub const AGENT_ENV: &str = "PI_SUBAGENT_AGENT";
/// Bus channel that ends a `sleep` early when a message arrives.
pub const WAKE_CHANNEL: &str = "agent:wake";
const POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Letter {
    pub from: String,
    /// The sender's agent name, or `parent`.
    pub agent: String,
    pub text: String,
    pub sent_at: u64,
}

#[derive(Debug, Deserialize)]
pub struct Params {
    pub to: String,
    pub message: String,
}

pub fn mailbox_root(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext/subagents/mailbox")
}

/// This process's address: the run id in a subagent, else the session id.
pub fn own_address(host: &Host) -> Option<String> {
    env_value(RUN_ENV).or_else(|| host.snapshot().and_then(|s| s.session_id.clone()))
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Writes `letter` into the mailbox of `address` with a temp-file rename, so the reader never
/// sees a partial file.
pub fn post(root: &Path, address: &str, letter: &Letter) -> std::io::Result<PathBuf> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let dir = root.join(address);
    std::fs::create_dir_all(&dir)?;
    let name = format!(
        "{:013}-{}-{}.json",
        letter.sent_at,
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let tmp = dir.join(format!(".{name}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec(letter)?)?;
    let path = dir.join(name);
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

/// The letters waiting in `address`'s mailbox, oldest first, with their files.
pub fn pending(root: &Path, address: &str) -> Vec<(PathBuf, Letter)> {
    let Ok(entries) = std::fs::read_dir(root.join(address)) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "json")
                && !path
                    .file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let letter = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            if letter.is_none() {
                pi_ext_sdk::warn!("dropping unreadable agent message {}", path.display());
                let _ = std::fs::remove_file(&path);
            }
            letter.map(|letter| (path, letter))
        })
        .collect()
}

/// The text the recipient sees, with how to reply.
pub fn delivery_text(letter: &Letter) -> String {
    format!(
        "<agent-message from=\"{}\" agent=\"{}\">\n{}\n</agent-message>\nReply with {TOOL} to \"{}\" when an answer is needed.",
        letter.from, letter.agent, letter.text, letter.from
    )
}

pub fn spec() -> ToolSpec {
    ToolSpec::new(
        TOOL,
        "Send a message to another agent: `parent` from a subagent, or a subagent run id. The recipient \
sees it at its next step, and an idle recipient starts a turn. Replies arrive the same way; do not sleep or poll for them.",
    )
    .label("Agent Message")
    .parameters(json!({
        "type": "object",
        "properties": {
            "to": { "type": "string", "description": "`parent`, or the run id of a running subagent" },
            "message": { "type": "string", "description": "Message text" }
        },
        "required": ["to", "message"]
    }))
    .prompt_snippet("Message the parent agent or a running subagent; replies wake you")
}

/// Resolves `to` to a mailbox address, or the reason the message cannot be sent.
fn recipient(app: &App, host: &Host, root: &Path, to: &str) -> std::result::Result<String, String> {
    let to = to.trim();
    if to == "parent" {
        return env_value(PARENT_ENV).ok_or_else(|| {
            "This agent has no parent; `parent` works only in a subagent.".to_owned()
        });
    }
    if let Some(run) = app.runs.get(to) {
        return match run.status {
            Status::Running => Ok(to.to_owned()),
            status => Err(format!("Run {to} is not running ({}).", status.as_str())),
        };
    }
    // A sibling run of the same parent has a mailbox while it runs.
    if env_value(PARENT_ENV).is_some()
        && root.join(to).is_dir()
        && own_address(host).as_deref() != Some(to)
    {
        return Ok(to.to_owned());
    }
    Err(format!(
        "No running agent {to}. Use `parent` or the id of a running subagent run."
    ))
}

pub async fn execute(app: App, params: Params, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host().clone();
    if params.message.trim().is_empty() {
        return Ok(ToolResult::error("message is empty."));
    }
    let root = mailbox_root(&host.agent_dir());
    let address = match recipient(&app, &host, &root, &params.to) {
        Ok(address) => address,
        Err(reason) => return Ok(ToolResult::error(reason)),
    };
    let letter = Letter {
        from: own_address(&host).unwrap_or_else(|| "unknown".into()),
        agent: env_value(AGENT_ENV).unwrap_or_else(|| "parent".into()),
        text: params.message,
        sent_at: now_ms(),
    };
    match post(&root, &address, &letter) {
        Ok(_) => Ok(ToolResult::text(format!("Sent to {}.", params.to.trim()))),
        Err(err) => Ok(ToolResult::error(format!("Cannot send: {err}"))),
    }
}

/// Registers `agent_message` and delivers this process's mail until shutdown. A letter's file
/// is removed only after pi accepted the message, so mail survives a failed send or a reload.
pub async fn serve(app: App, host: Host) -> Result<()> {
    if !app.config(&host).messaging {
        return Ok(());
    }
    let tool_app = app.clone();
    host.register_tool(spec(), move |params: Params, ctx: ToolCtx| {
        execute(tool_app.clone(), params, ctx)
    })
    .await?;
    let root = mailbox_root(&host.agent_dir());
    tokio::spawn(async move {
        let mut current: Option<String> = None;
        loop {
            let address = own_address(&host);
            if address != current {
                if let Some(address) = &address {
                    let _ = std::fs::create_dir_all(root.join(address));
                }
                current = address;
            }
            if let Some(address) = &current {
                deliver(&host, &root, address).await;
            }
            tokio::time::sleep(POLL).await;
        }
    });
    Ok(())
}

async fn deliver(host: &Host, root: &Path, address: &str) {
    for (path, letter) in pending(root, address) {
        let message = CustomMessage::new(MESSAGE_TYPE, delivery_text(&letter))
            .display(true)
            .details(&letter)
            .deliver_as(DeliverAs::Steer)
            .trigger_turn(true);
        if let Err(err) = host.send_message(message).await {
            pi_ext_sdk::debug!("agent message not delivered yet: {}", err.message());
            return;
        }
        let _ = std::fs::remove_file(&path);
        let _ = host
            .emit(WAKE_CHANNEL, json!({ "source": "agent-message", "reason": format!("message from {}", letter.from) }))
            .await;
    }
}

/// Removes this subagent's mailbox at exit, so siblings stop addressing it.
pub fn close(host: &Host) {
    if let Some(run) = env_value(RUN_ENV) {
        let dir = mailbox_root(&host.agent_dir()).join(run);
        if pending_is_empty(&dir) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

fn pending_is_empty(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn letter(text: &str, sent_at: u64) -> Letter {
        Letter {
            from: "run1".into(),
            agent: "build".into(),
            text: text.into(),
            sent_at,
        }
    }

    #[test]
    fn posted_letters_come_back_oldest_first() {
        let root = std::env::temp_dir().join(format!("pi-mailbox-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        post(&root, "parent-session", &letter("second", 20)).unwrap();
        post(&root, "parent-session", &letter("first", 10)).unwrap();
        std::fs::write(root.join("parent-session/.partial.json.tmp"), b"{").unwrap();
        let texts: Vec<String> = pending(&root, "parent-session")
            .into_iter()
            .map(|(_, letter)| letter.text)
            .collect();
        assert_eq!(texts, ["first", "second"]);
        assert!(pending(&root, "nobody").is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn delivery_text_names_the_sender_and_how_to_reply() {
        let text = delivery_text(&letter("tests pass", 1));
        assert!(
            text.starts_with("<agent-message from=\"run1\" agent=\"build\">\ntests pass\n"),
            "{text}"
        );
        assert!(
            text.ends_with("Reply with agent_message to \"run1\" when an answer is needed."),
            "{text}"
        );
    }
}
