//! Durable background shell jobs: `bg_run`, `bg_status`, `bg_logs`, `bg_kill`, `/tasks`.

mod command;
mod identity;
mod panel;
mod process;
mod registry;
mod task;
mod tools;
mod ui;

#[cfg(test)]
mod process_tests;
#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod test_host;
#[cfg(test)]
mod tests;

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

use registry::Registry;

const POLICY_SECTION: &str = "pi_background_shell_policy";
const POLICY: &str = "Background shell policy: start long-running commands (test suites, builds, dev servers, watchers) with bg_run instead of bash. bg_run returns at once; a <background-task-notification> reports the terminal state and starts a follow-up turn. Do not sleep or poll bg_status/bg_logs to wait. Stop tasks that are no longer needed with bg_kill.";

pub fn main() -> ExitCode {
    extension().run()
}

/// Binds the registry to the session directory and starts the monitor, once.
pub(crate) fn ensure(host: &Host, snapshot: &Snapshot, registry: &Arc<Registry>) {
    if !registry.is_bound() {
        let session = snapshot
            .session_id
            .clone()
            .or_else(|| host.snapshot().and_then(|s| s.session_id.clone()));
        registry.bind(task::session_dir(&host.agent_dir(), session.as_deref()));
    }
    ui::spawn_monitor(host.clone(), registry.clone());
}

#[derive(Deserialize, Default)]
struct ShutdownEvent {
    #[serde(default)]
    reason: String,
}

pub fn extension() -> Extension {
    let registry = Arc::new(Registry::default());
    let ext = Extension::new("background-tasks");
    let ext = tools::register(ext, registry.clone());
    let ext = command::register(ext, registry.clone());
    let (start, shutdown, surface) = (registry.clone(), registry.clone(), registry);
    ext.on("before_agent_start", |_ev: EventCtx| async move {
        Ok(EventReply::section(POLICY_SECTION, POLICY))
    })
    .on("session_shutdown", move |ev: EventCtx| {
        let registry = shutdown.clone();
        async move {
            let event: ShutdownEvent = ev.event_as().unwrap_or_default();
            let stopped = registry.shutdown(&event.reason).await;
            if stopped > 0 {
                pi_ext_sdk::info!("stopped {stopped} background task(s) on {}", event.reason);
            }
            Ok(())
        }
    })
    .on_start(move |host: Host| {
        let registry = start.clone();
        async move {
            let snapshot = host.snapshot().map(|s| (*s).clone()).unwrap_or_default();
            ensure(&host, &snapshot, &registry);
            ui::refresh(&host, &registry).await
        }
    })
    .on_surface(move |event: SurfaceEvent, host: Host| {
        let registry = surface.clone();
        async move {
            match event {
                SurfaceEvent::Resize { .. } if event.widget_key() == Some(ui::WIDGET) => {
                    ui::refresh(&host, &registry).await
                }
                _ => Ok(()),
            }
        }
    })
}
