use std::sync::Arc;

use pi_ext_sdk::{CommandCtx, Host, Result};
use serde_json::{Value, json};

use crate::checkpoint::{self, Checkpoint};
use crate::lifecycle;
use crate::state::State;

pub const BOTH: &str = "Restore code and conversation";
pub const CONVERSATION: &str = "Restore conversation";
pub const CODE: &str = "Restore code";
pub const CURRENT: &str = "(current)";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Both,
    Conversation,
    Code,
}

impl Mode {
    pub fn parse(label: &str) -> Option<Self> {
        match label {
            BOTH => Some(Self::Both),
            CONVERSATION => Some(Self::Conversation),
            CODE => Some(Self::Code),
            _ => None,
        }
    }

    fn code(self) -> bool {
        matches!(self, Self::Both | Self::Code)
    }

    fn conversation(self) -> bool {
        matches!(self, Self::Both | Self::Conversation)
    }
}

pub fn mode_options(code_differs: bool) -> Vec<&'static str> {
    if code_differs {
        vec![BOTH, CONVERSATION, CODE]
    } else {
        vec![CONVERSATION]
    }
}

pub async fn run(state: Arc<State>, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    if !host.has_ui() {
        return host.notify_error("/rewind needs the interactive UI.").await;
    }
    lifecycle::finish(&state, &host).await;
    let checkpoints = checkpoint::checkpoints(&host.branch(None).await?);
    if checkpoints.is_empty() {
        return host.notify_info("No checkpoints on this branch yet.").await;
    }
    let mut options: Vec<String> = checkpoints
        .iter()
        .enumerate()
        .map(|(index, cp)| checkpoint::label(index, cp))
        .collect();
    options.push(CURRENT.to_string());
    let Some(choice) = host.select("Rewind to checkpoint:", &options).await? else {
        return Ok(());
    };
    let Some(index) = options.iter().position(|option| *option == choice) else {
        return Ok(());
    };
    let Some(target) = checkpoints.get(index) else {
        return Ok(());
    };
    let code_differs = code_differs(&state, &checkpoints, index).await;
    let modes = mode_options(code_differs);
    let Some(mode) = host.select("Restore mode:", &modes).await? else {
        return Ok(());
    };
    let Some(mode) = Mode::parse(&mode) else {
        return Ok(());
    };
    apply(&state, &host, target, mode).await
}

async fn code_differs(state: &State, checkpoints: &[Checkpoint], index: usize) -> bool {
    if checkpoint::files_changed_since(checkpoints, index) > 0 {
        return true;
    }
    let mut guard = state.lock().await;
    let Some(session) = guard.as_mut().filter(|s| s.config.enabled) else {
        return false;
    };
    let commit = &checkpoints[index].before_commit;
    session.repo.has_commit(commit).await && !session.matches(commit).await.unwrap_or(true)
}

pub async fn apply(state: &State, host: &Host, target: &Checkpoint, mode: Mode) -> Result<()> {
    let mut code_error = None;
    if mode.code() {
        let mut guard = state.lock().await;
        code_error = match guard.as_mut() {
            Some(session) if session.config.enabled => session
                .restore(&target.before_commit)
                .await
                .err()
                .map(|e| e.to_string()),
            _ => Some("checkpoints are disabled".to_string()),
        };
    }
    if mode.conversation() {
        state.set_suppress_tree(true);
        let options = json!({ "summarize": false });
        let args = vec![Value::String(target.user_entry_id.clone()), options];
        let navigated = host.pi_call("ctx", "navigateTree", args).await;
        state.set_suppress_tree(false);
        let result = navigated?;
        if result["cancelled"] == true || result["aborted"] == true {
            return host
                .notify_warning("Conversation restore was cancelled.")
                .await;
        }
        let editor_text = result["editorText"].as_str().unwrap_or(&target.prompt);
        host.set_editor_text(editor_text).await?;
    }
    let prompt = checkpoint::short_prompt(&target.prompt, 60);
    match code_error {
        Some(err) => {
            let text = format!("files were not restored: {err}");
            lifecycle::show_warning(state, host, &text).await
        }
        None => {
            let done = match mode {
                Mode::Both => "Restored code and conversation",
                Mode::Conversation => "Restored conversation",
                Mode::Code => "Restored code",
            };
            host.notify_info(format!("{done} to: {prompt}")).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_conversation_only_without_changes() {
        assert_eq!(mode_options(false), [CONVERSATION]);
        assert_eq!(mode_options(true), [BOTH, CONVERSATION, CODE]);
        assert_eq!(Mode::parse(CODE), Some(Mode::Code));
        assert!(Mode::Both.code() && Mode::Both.conversation());
        assert!(!Mode::Code.conversation() && !Mode::Conversation.code());
        assert_eq!(Mode::parse("x"), None);
    }
}
