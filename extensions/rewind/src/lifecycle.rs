use std::path::Path;
use std::sync::Arc;

use pi_ext_sdk::event_types::SessionStartEvent;
use pi_ext_sdk::{EventCtx, Host, Result, warn};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::checkpoint;
use crate::config;
use crate::git::ShadowRepo;
use crate::state::{Session, State, latest_after, session_key, sessions_dir, storage_dir};

pub const WARNING_KEY: &str = "pi-rewind-code-restore-warning";
const FORK_INTENT: &str = "fork-intent.json";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ForkIntent {
    pub entry_id: String,
    pub position: String,
    pub before_commit: Option<String>,
}

pub async fn session_start(state: Arc<State>, ev: EventCtx) -> Result<()> {
    let host = ev.host().clone();
    let start: SessionStartEvent = ev.event_as().unwrap_or_default();
    let trusted = ev.snapshot().extra.get("isProjectTrusted") == Some(&Value::Bool(true));
    let (config, notes) = config::load(&host, trusted).await;
    for note in notes {
        host.notify_warning(note).await?;
    }
    let storage = storage_dir(&host.agent_dir(), ev.snapshot());
    let mut session = Session::new(config, storage, &host.cwd());
    let branch = host.branch(None).await.unwrap_or_default();
    session.last_commit = latest_after(&branch);
    let restore = match start.reason.as_str() {
        "fork" => {
            let previous = start.previous_session_file.as_deref();
            fork_restore(&mut session, &host.agent_dir(), previous, &branch).await
        }
        "resume" if session.config.restore_on_resume => session.last_commit.clone(),
        _ => None,
    };
    let has_checkpoints = !checkpoint::checkpoints(&branch).is_empty();
    if has_checkpoints && !session.repo.exists() {
        let text = "Checkpoint storage for this session is missing; file restore is unavailable.";
        host.notify_warning(text).await?;
    }
    if let Some(commit) = restore.filter(|_| session.config.enabled) {
        match session.restore(&commit).await {
            Ok(()) => {
                host.notify_info("Restored files to the session checkpoint.")
                    .await?
            }
            Err(err) => {
                let text = format!("File restore failed: {}", err);
                host.notify_warning(text).await?
            }
        }
    }
    *state.lock().await = Some(session);
    Ok(())
}

/// Copies the parent session's storage and returns the commit to restore, if any.
async fn fork_restore(
    session: &mut Session,
    agent_dir: &Path,
    previous_file: Option<&str>,
    branch: &[Value],
) -> Option<String> {
    let previous_file = previous_file.filter(|file| !file.is_empty())?;
    let source_dir = sessions_dir(agent_dir).join(session_key(Some(previous_file), None));
    let source = ShadowRepo::new(&source_dir, Path::new("."));
    if source.exists()
        && !session.repo.exists()
        && let Err(err) = session.repo.clone_from(&source).await
    {
        warn!("could not copy checkpoint storage: {}", err);
    }
    let intent_path = source_dir.join(FORK_INTENT);
    let intent: ForkIntent = std::fs::read_to_string(&intent_path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())?;
    let _ = std::fs::remove_file(&intent_path);
    match intent.position.as_str() {
        "at" if session.config.restore_on_clone => latest_after(branch),
        "before" if session.config.restore_on_fork => intent.before_commit,
        _ => None,
    }
}

/// `session_compact`: re-reads the diff base from the branch; files and the pending turn stay.
pub async fn session_compact(state: Arc<State>, ev: EventCtx) -> Result<()> {
    let branch = ev.host().branch(None).await?;
    if let Some(commit) = latest_after(&branch)
        && let Some(session) = state.lock().await.as_mut()
    {
        session.last_commit = Some(commit);
    }
    Ok(())
}

pub async fn message_start(state: Arc<State>, ev: EventCtx) -> Result<()> {
    if ev.event()["message"]["role"] != "assistant" {
        return Ok(());
    }
    let host = ev.host().clone();
    let branch = host.branch(None).await?;
    let Some((user_entry_id, prompt)) = checkpoint::latest_user_entry(&branch) else {
        return Ok(());
    };
    let mut guard = state.lock().await;
    let Some(session) = guard.as_mut().filter(|s| s.config.checkpoints_on()) else {
        return Ok(());
    };
    if let Err(err) = session.begin_turn(&host, &user_entry_id, &prompt).await {
        warn!("checkpoint before turn failed: {}", err);
    }
    Ok(())
}

pub async fn finish(state: &State, host: &Host) {
    let mut guard = state.lock().await;
    if let Some(session) = guard.as_mut()
        && let Err(err) = session.finish_turn(host).await
    {
        warn!("checkpoint after turn failed: {}", err);
    }
}

pub async fn before_fork(state: Arc<State>, ev: EventCtx) -> Result<()> {
    let host = ev.host().clone();
    finish(&state, &host).await;
    let entry_id = ev.event()["entryId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let position = ev.event()["position"]
        .as_str()
        .unwrap_or("before")
        .to_string();
    let entries = host.entries().await.unwrap_or_default();
    let before_commit = checkpoint::checkpoints(&entries)
        .into_iter()
        .find(|cp| cp.user_entry_id == entry_id)
        .map(|cp| cp.before_commit);
    let guard = state.lock().await;
    let Some(session) = guard.as_ref().filter(|s| s.repo.exists()) else {
        return Ok(());
    };
    let intent = ForkIntent {
        entry_id,
        position,
        before_commit,
    };
    std::fs::create_dir_all(&session.storage)?;
    std::fs::write(
        session.storage.join(FORK_INTENT),
        serde_json::to_string(&intent)?,
    )?;
    Ok(())
}

pub async fn input(state: Arc<State>, ev: EventCtx) -> Result<()> {
    if state.set_warning(false) {
        ev.host().clear_widget(WARNING_KEY).await?;
    }
    Ok(())
}

pub async fn show_warning(state: &State, host: &Host, text: &str) -> Result<()> {
    state.set_warning(true);
    let line = pi_ext_sdk::style::fg(
        pi_ext_sdk::style::Color::Yellow,
        &format!("Warning: {text}"),
    );
    host.set_widget(WARNING_KEY, [line], pi_ext_sdk::Placement::AboveEditor)
        .await
}
