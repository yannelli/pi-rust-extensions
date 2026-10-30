//! The `ui.custom` panel loop: keys and resizes in, lines out, first answer wins.

use pi_ext_sdk::{CancellationToken, Error, Host, PanelEvent, Result};
use serde_json::Value;

use crate::channel::OpenQuestionnaire;
use crate::render::render;
use crate::result::QuestionnaireResult;
use crate::state::{EditTarget, Effect, Questionnaire};

pub const DEFAULT_WIDTH: usize = 80;

/// `Ok(None)` when pi cannot show a panel; the caller falls back to dialogs.
pub async fn run_panel(
    host: &Host,
    cancel: &CancellationToken,
    open: &OpenQuestionnaire,
    mut state: Questionnaire,
) -> Result<Option<QuestionnaireResult>> {
    let opened = host.panel(render(&state, DEFAULT_WIDTH)).open().await;
    let mut panel = match opened {
        Ok(panel) => panel,
        Err(err) if err.is_no_ui() => return Ok(None),
        Err(err) => return Err(err),
    };
    let mut width = panel.width().unwrap_or(DEFAULT_WIDTH);
    loop {
        let event = tokio::select! {
            result = open.answered() => {
                panel.close(Value::Null).await?;
                return Ok(Some(result));
            }
            () = cancel.cancelled() => {
                drop(panel);
                return Err(Error::cancelled());
            }
            event = panel.next_event() => event,
        };
        let effect = match &event {
            PanelEvent::Resize(w) => {
                width = *w;
                None
            }
            PanelEvent::Key(key) => state.handle_key(key),
            PanelEvent::Closed => Some(state.done(true)),
        };
        match effect {
            Some(Effect::Done(result)) => {
                open.settle(result);
                let accepted = open.settled().unwrap_or_default();
                if !matches!(event, PanelEvent::Closed) {
                    panel.close(Value::Null).await?;
                }
                return Ok(Some(accepted));
            }
            Some(Effect::Edit(target, draft)) => {
                let title = match target {
                    EditTarget::Input => "Edit answer",
                    EditTarget::Notes => "Edit note",
                };
                match host.editor(title, Some(&draft)).await {
                    Ok(text) => state.apply_edit(target, text),
                    Err(err) => {
                        let message = format!("External editor failed: {}", err.message());
                        let _ = host.notify_error(message).await;
                    }
                }
            }
            None => {}
        }
        panel.set_lines(render(&state, width))?;
    }
}
