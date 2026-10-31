//! The compactor's `[Active Work]` block of the running subagent runs on `compactor:active-work`.
//! Each message names its session, so a late answer cannot outlive a switch.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use crate::app::App;
use crate::runs::{Run, Status, first_line};

pub const CHANNEL: &str = "compactor:active-work";
pub const SYNC: &str = "compactor:active-work:sync";
pub const ID: &str = "subagents";
const SHOWN: usize = 10;

/// The session and text of the last message.
type Sent = Option<(Option<String>, String)>;

/// The text last sent, with its session, so an unchanged state sends nothing.
#[derive(Clone, Default)]
pub struct Published(Arc<Mutex<Sent>>);

impl Published {
    /// Sends `text()` unless this session already has it; `force` sends it anyway. The lock spans
    /// reading and sending, so the last message always has the newest state.
    pub async fn publish(&self, host: &Host, text: impl FnOnce() -> String, force: bool) {
        let mut last = self.0.lock().await;
        let session = host.snapshot().and_then(|s| s.session_id.clone());
        let key = (session.clone(), text());
        if !force && last.as_ref() == Some(&key) {
            return;
        }
        let data = json!({ "id": ID, "text": key.1, "sessionId": session });
        *last = Some(key);
        if let Err(err) = host.emit(CHANNEL, data).await {
            pi_ext_sdk::debug!("cannot publish active work: {err}");
        }
    }
}

/// One line per running run, without times or progress, so a running run does not change it.
pub fn text(runs: &[Run]) -> String {
    let running: Vec<&Run> = runs
        .iter()
        .filter(|run| run.status == Status::Running)
        .collect();
    if running.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!("Subagents running ({}):", running.len())];
    for run in running.iter().take(SHOWN) {
        let mode = if run.background { " (background)" } else { "" };
        lines.push(format!(
            "- {} {}{mode}: {}",
            run.id,
            run.agent,
            first_line(&run.task, 120)
        ));
    }
    if running.len() > SHOWN {
        lines.push(format!(
            "- and {} more (/agents runs lists them)",
            running.len() - SHOWN
        ));
    }
    lines.join("\n")
}

pub async fn publish(app: &App, host: &Host, force: bool) {
    app.published
        .publish(host, || text(&app.runs.all()), force)
        .await;
}

/// Sends the current block once, then again for each sync request of the compactor.
pub async fn serve(app: App, host: Host) -> Result<()> {
    let mut sync = host.subscribe(SYNC).await?;
    publish(&app, &host, true).await;
    while sync.recv().await.is_some() {
        publish(&app, &host, true).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runs::Runs;
    use pi_ext_sdk::CancellationToken;

    #[test]
    fn running_runs_show_and_finished_runs_leave() {
        let runs = Runs::default();
        let first = runs.start(
            "reviewer",
            "Review the parser\nin detail",
            true,
            CancellationToken::new(),
        );
        let second = runs.start("scout", "Find callers", false, CancellationToken::new());
        let text_of = || text(&runs.all());
        assert_eq!(
            text_of(),
            format!(
                "Subagents running (2):\n- {first} reviewer (background): Review the parser\n- {second} scout: Find callers"
            )
        );
        runs.finish(&second, Status::Completed, 10);
        runs.finish(&first, Status::Failed, 10);
        assert_eq!(text_of(), "");
    }
}
