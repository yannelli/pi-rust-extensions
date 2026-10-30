use serde::Serialize;
use serde_json::{Value, json};
use tokio::sync::{mpsc, oneshot};

use crate::error::{Error, Result, code};
use crate::host::Host;
use crate::keys::Key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelEvent {
    Key(Key),
    Resize(usize),
    /// The panel closed: pi removed it, or `ui.custom` answered.
    Closed,
}

/// Options for a `ui.custom` panel; `Host::panel` creates it.
pub struct PanelBuilder {
    host: Host,
    lines: Vec<String>,
    surface: Option<String>,
    overlay: bool,
    overlay_options: Option<Value>,
}

impl PanelBuilder {
    pub(crate) fn new(host: Host, lines: Vec<String>) -> Self {
        Self {
            host,
            lines,
            surface: None,
            overlay: false,
            overlay_options: None,
        }
    }

    pub fn surface(mut self, surface: impl Into<String>) -> Self {
        self.surface = Some(surface.into());
        self
    }

    pub fn overlay(mut self, overlay: bool) -> Self {
        self.overlay = overlay;
        self
    }

    /// Host overlay options, for example `{ "width": "60%", "anchor": "center" }`.
    pub fn overlay_options(mut self, options: Value) -> Self {
        self.overlay = true;
        self.overlay_options = Some(options);
        self
    }

    /// Sends `ui.custom` and returns at once; the request stays pending until the panel closes.
    pub async fn open(self) -> Result<Panel> {
        if !self.host.has_ui() {
            return Err(Error::new(code::NO_UI, "pi has no UI"));
        }
        let surface = self.surface.unwrap_or_else(|| self.host.next_surface_id());
        let mut params = json!({ "surface": surface, "lines": self.lines });
        if self.overlay {
            params["overlay"] = json!(true);
        }
        if let Some(options) = self.overlay_options {
            params["overlayOptions"] = options;
        }
        let (events_tx, events) = mpsc::unbounded_channel();
        self.host.surfaces().add_panel(&surface, events_tx);
        let call = match self.host.start_call("ui.custom", params) {
            Ok(call) => call,
            Err(err) => {
                self.host.surfaces().remove_panel(&surface);
                return Err(err);
            }
        };
        let (result_tx, result) = oneshot::channel();
        let host = self.host.clone();
        let id = surface.clone();
        tokio::spawn(async move {
            let answer = call.wait().await;
            host.surfaces().remove_panel(&id);
            let answer = answer.map(|v| v.get("result").cloned().unwrap_or(Value::Null));
            let _ = result_tx.send(answer);
        });
        Ok(Panel {
            host: self.host,
            surface,
            events,
            result: Some(result),
            ended: false,
        })
    }
}

/// An open `ui.custom` panel. Dropping it without `close` sends `ui.close`.
pub struct Panel {
    host: Host,
    surface: String,
    events: mpsc::UnboundedReceiver<PanelEvent>,
    result: Option<oneshot::Receiver<Result<Value>>>,
    ended: bool,
}

impl Panel {
    pub fn surface(&self) -> &str {
        &self.surface
    }

    /// The last width from `ui.resize`, when pi sent one.
    pub fn width(&self) -> Option<usize> {
        self.host.surfaces().width(&self.surface)
    }

    /// Replaces the panel lines (`ui.update`).
    pub fn set_lines<I, S>(&self, lines: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let lines: Vec<String> = lines.into_iter().map(Into::into).collect();
        let params = json!({ "surface": self.surface, "lines": lines });
        self.host.notify("ui.update", params)
    }

    /// The next key, resize, or `Closed`. After `Closed` it keeps returning `Closed`.
    pub async fn next_event(&mut self) -> PanelEvent {
        let event = self.events.recv().await;
        self.ended = event.is_none();
        event.unwrap_or(PanelEvent::Closed)
    }

    /// Sends `ui.close` with `result` and waits for `ui.custom` to answer.
    pub async fn close(mut self, result: impl Serialize) -> Result<Value> {
        let result = serde_json::to_value(result)?;
        let params = json!({ "surface": self.surface, "result": result });
        self.host.notify("ui.close", params)?;
        self.wait().await
    }

    /// Waits until the panel closes and returns the `result` of `ui.custom`.
    pub async fn result(mut self) -> Result<Value> {
        self.wait().await
    }

    async fn wait(&mut self) -> Result<Value> {
        let rx = self.result.take().ok_or_else(Error::closed)?;
        rx.await.unwrap_or_else(|_| Err(Error::closed()))
    }
}

impl Drop for Panel {
    fn drop(&mut self) {
        let open = !self.ended
            && self.result.as_mut().is_some_and(|rx| {
                matches!(rx.try_recv(), Err(oneshot::error::TryRecvError::Empty))
            });
        if open {
            let params = json!({ "surface": self.surface, "result": Value::Null });
            let _ = self.host.notify("ui.close", params);
        }
    }
}
