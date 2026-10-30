//! Shared extension state: runs, config and agent loading, and the background widget.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use pi_ext_sdk::prelude::*;

use crate::agent::AgentDef;
use crate::config::{self, Config};
use crate::registry::{self, Layers};
use crate::render;
use crate::runs::{Runs, Status};

pub const WIDGET: &str = "subagents";

#[derive(Clone, Default)]
pub struct App {
    pub runs: Runs,
    widget_ticking: Arc<AtomicBool>,
    widget_shown: Arc<AtomicBool>,
    warned_untrusted: Arc<AtomicBool>,
}

/// `isProjectTrusted` from the ctx snapshot; `false` when pi does not send it.
pub fn project_trusted(host: &Host) -> bool {
    host.snapshot()
        .and_then(|s| s.extra.get("isProjectTrusted").and_then(Value::as_bool))
        .unwrap_or(false)
}

impl App {
    pub fn layers(&self, host: &Host) -> Layers {
        let trusted = project_trusted(host);
        let project = host.project_dir();
        Layers::new(&host.agent_dir(), trusted.then_some(project.as_path()))
    }

    pub fn agents(&self, host: &Host) -> Vec<AgentDef> {
        let layers = self.layers(host);
        if layers.project.is_none() && registry::agents_dir(&host.project_dir()).is_dir() {
            self.warn_untrusted(host, "project agents in .pi/ext/subagents/agents");
        }
        registry::load(&layers)
    }

    pub fn config(&self, host: &Host) -> Config {
        let loaded = config::load(
            &host.agent_dir(),
            &host.project_dir(),
            project_trusted(host),
        );
        if !loaded.ignored.is_empty() {
            let keys = loaded.ignored.join(", ");
            self.warn_untrusted(host, &format!("project subagents.json keys {keys}"));
        }
        loaded.config
    }

    fn warn_untrusted(&self, host: &Host, what: &str) {
        if self.warned_untrusted.swap(true, Ordering::Relaxed) {
            return;
        }
        let message = format!("subagents: the project is not trusted; ignoring {what}");
        pi_ext_sdk::warn!("{message}");
        let host = host.clone();
        tokio::spawn(async move {
            if host.has_ui() {
                let _ = host.notify_warning(message).await;
            }
        });
    }

    /// Redraws the widget once a second while background runs are running.
    pub fn start_widget(&self, host: &Host) {
        if !host.has_ui() || !self.config(host).widget {
            return;
        }
        if self.widget_ticking.swap(true, Ordering::SeqCst) {
            return;
        }
        let app = self.clone();
        let host = host.clone();
        tokio::spawn(async move {
            loop {
                let shown = app.draw_widget(&host).await;
                if !shown {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            app.widget_ticking.store(false, Ordering::SeqCst);
            let running = app
                .runs
                .all()
                .iter()
                .any(|r| r.background && r.status == Status::Running);
            if running {
                app.start_widget(&host);
            }
        });
    }

    /// Draws the widget; `false` (after clearing it) when no background run is running.
    pub async fn draw_widget(&self, host: &Host) -> bool {
        let width = host
            .surface_width(&format!("widget:{WIDGET}"))
            .unwrap_or(100);
        let lines = render::widget_lines(&self.runs.all(), width);
        if lines.is_empty() {
            if self.widget_shown.swap(false, Ordering::SeqCst) {
                let _ = host.clear_widget(WIDGET).await;
            }
            return false;
        }
        self.widget_shown.store(true, Ordering::SeqCst);
        let _ = host.set_widget(WIDGET, lines, Placement::BelowEditor).await;
        true
    }
}
