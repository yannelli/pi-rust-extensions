//! The compactor's `[Active Work]` block of the running background tasks on
//! `compactor:active-work`. Each message names its session, so a late answer cannot outlive a switch.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use crate::registry::Registry;
use crate::task::Task;

pub const CHANNEL: &str = "compactor:active-work";
pub const SYNC: &str = "compactor:active-work:sync";
pub const ID: &str = "background-tasks";
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

fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    if line.chars().count() <= max {
        return line.to_owned();
    }
    let cut: String = line.chars().take(max.saturating_sub(1)).collect();
    format!("{cut}…")
}

/// One line per running task, without times, so a running task does not change the text.
pub fn text(tasks: &[Task]) -> String {
    let running: Vec<&Task> = tasks
        .iter()
        .filter(|task| task.status.is_running())
        .collect();
    if running.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!("Background tasks running ({}):", running.len())];
    for task in running.iter().take(SHOWN) {
        let what = task.description.as_deref().unwrap_or(&task.command);
        lines.push(format!(
            "- {} {}: {}",
            task.id,
            task.name,
            one_line(what, 120)
        ));
    }
    if running.len() > SHOWN {
        lines.push(format!(
            "- and {} more (bg_status lists them)",
            running.len() - SHOWN
        ));
    }
    lines.join("\n")
}

pub async fn publish(host: &Host, registry: &Registry, force: bool) {
    registry
        .published
        .publish(host, || text(&registry.tasks()), force)
        .await;
}

/// Sends the current block once, then again for each sync request of the compactor.
pub async fn serve(host: Host, registry: Arc<Registry>) -> Result<()> {
    let mut sync = host.subscribe(SYNC).await?;
    publish(&host, &registry, true).await;
    while sync.recv().await.is_some() {
        publish(&host, &registry, true).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Status;
    use crate::tests::sample;

    #[test]
    fn only_running_tasks_show_and_the_text_has_no_times() {
        let mut running = sample("t1", Status::Running);
        running.name = "Watch".into();
        running.command = "cargo watch\n--clear".into();
        let tasks = [
            running,
            sample("t2", Status::Completed),
            sample("t3", Status::Killed),
        ];
        assert_eq!(
            text(&tasks),
            "Background tasks running (1):\n- t1 Watch: cargo watch"
        );
        assert_eq!(text(&tasks[1..]), "");
    }
}
