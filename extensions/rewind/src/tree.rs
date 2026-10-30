use std::sync::Arc;

use pi_ext_sdk::{EventCtx, Result};

use crate::checkpoint;
use crate::config::RestoreOnTree;
use crate::lifecycle;
use crate::state::State;

/// `session_before_tree`: decides whether `/tree` navigation also syncs files.
pub async fn before_tree(state: Arc<State>, ev: EventCtx) -> Result<()> {
    if state.tree_suppressed() {
        return Ok(());
    }
    let preparation = &ev.event()["preparation"];
    if preparation["userWantsSummary"].as_bool() == Some(true) {
        return Ok(());
    }
    let Some(target) = preparation["targetId"].as_str().map(String::from) else {
        return Ok(());
    };
    let host = ev.host().clone();
    lifecycle::finish(&state, &host).await;
    let mode = match state.lock().await.as_ref() {
        Some(session) if session.config.enabled => session.config.restore_on_tree,
        _ => return Ok(()),
    };
    let sync = match mode {
        RestoreOnTree::Never => false,
        RestoreOnTree::Always => true,
        RestoreOnTree::Ask if host.has_ui() => host
            .confirm(
                "Sync files?",
                "Restore workspace files to the selected point?",
            )
            .await
            .unwrap_or(false),
        RestoreOnTree::Ask => false,
    };
    if let Some(session) = state.lock().await.as_mut() {
        session.tree_target = sync.then_some(Some(target));
    }
    Ok(())
}

/// `session_tree`: applies the restore planned in `before_tree`.
pub async fn after_tree(state: Arc<State>, ev: EventCtx) -> Result<()> {
    if state.tree_suppressed() {
        return Ok(());
    }
    let host = ev.host().clone();
    let mut guard = state.lock().await;
    let Some(session) = guard.as_mut() else {
        return Ok(());
    };
    let Some(target) = session.tree_target.take() else {
        return Ok(());
    };
    let all = checkpoint::checkpoints(&host.entries().await?);
    let branch = checkpoint::checkpoints(&host.branch(None).await?);
    let Some(commit) = checkpoint::tree_target_commit(&all, &branch, target.as_deref()) else {
        drop(guard);
        return host
            .notify_info("No checkpoint for this point; files unchanged.")
            .await;
    };
    let result = session.restore(&commit).await;
    drop(guard);
    match result {
        Ok(()) => {
            host.notify_info("Files synced to the selected point.")
                .await
        }
        Err(err) => {
            let text = format!("files were not restored: {err}");
            lifecycle::show_warning(&state, &host, &text).await
        }
    }
}
