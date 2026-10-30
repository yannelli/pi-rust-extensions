use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::{broadcast, mpsc};

use crate::keys::Key;
use crate::panel::PanelEvent;

/// A change to a widget (`widget:<key>`) or the footer (`footer`) surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SurfaceEvent {
    Resize { surface: String, width: usize },
    Closed { surface: String },
}

impl SurfaceEvent {
    pub fn surface(&self) -> &str {
        match self {
            Self::Resize { surface, .. } | Self::Closed { surface } => surface,
        }
    }

    /// The widget key when the surface is `widget:<key>`.
    pub fn widget_key(&self) -> Option<&str> {
        self.surface().strip_prefix("widget:")
    }
}

/// Receiver returned by `Host::surface_events`.
pub struct SurfaceEvents {
    rx: broadcast::Receiver<SurfaceEvent>,
}

impl SurfaceEvents {
    /// The next event; `None` after the connection closes. Skips events lost to lag.
    pub async fn recv(&mut self) -> Option<SurfaceEvent> {
        loop {
            match self.rx.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }
}

pub(crate) struct Surfaces {
    panels: Mutex<HashMap<String, mpsc::UnboundedSender<PanelEvent>>>,
    widths: Mutex<HashMap<String, usize>>,
    events: broadcast::Sender<SurfaceEvent>,
}

impl Surfaces {
    pub(crate) fn new() -> Self {
        Self {
            panels: Mutex::new(HashMap::new()),
            widths: Mutex::new(HashMap::new()),
            events: broadcast::channel(64).0,
        }
    }

    pub(crate) fn add_panel(&self, surface: &str, tx: mpsc::UnboundedSender<PanelEvent>) {
        self.panels
            .lock()
            .expect("panels lock")
            .insert(surface.to_owned(), tx);
    }

    pub(crate) fn remove_panel(&self, surface: &str) {
        self.panels.lock().expect("panels lock").remove(surface);
        self.widths.lock().expect("widths lock").remove(surface);
    }

    fn panel(&self, surface: &str) -> Option<mpsc::UnboundedSender<PanelEvent>> {
        self.panels
            .lock()
            .expect("panels lock")
            .get(surface)
            .cloned()
    }

    pub(crate) fn width(&self, surface: &str) -> Option<usize> {
        self.widths
            .lock()
            .expect("widths lock")
            .get(surface)
            .copied()
    }

    pub(crate) fn subscribe(&self) -> SurfaceEvents {
        SurfaceEvents {
            rx: self.events.subscribe(),
        }
    }

    /// Routes a `ui.resize`; returns the event for non-panel surfaces.
    pub(crate) fn resize(&self, surface: &str, width: usize) -> Option<SurfaceEvent> {
        let mut widths = self.widths.lock().expect("widths lock");
        widths.insert(surface.to_owned(), width);
        drop(widths);
        if let Some(panel) = self.panel(surface) {
            let _ = panel.send(PanelEvent::Resize(width));
            return None;
        }
        let event = SurfaceEvent::Resize {
            surface: surface.to_owned(),
            width,
        };
        let _ = self.events.send(event.clone());
        Some(event)
    }

    pub(crate) fn key(&self, surface: &str, data: &str) {
        if let Some(panel) = self.panel(surface) {
            let _ = panel.send(PanelEvent::Key(Key::parse(data)));
        }
    }

    /// Routes a `ui.closed`; returns the event for non-panel surfaces.
    pub(crate) fn closed(&self, surface: &str) -> Option<SurfaceEvent> {
        if self.panel(surface).is_some() {
            self.remove_panel(surface);
            return None;
        }
        self.widths.lock().expect("widths lock").remove(surface);
        let event = SurfaceEvent::Closed {
            surface: surface.to_owned(),
        };
        let _ = self.events.send(event.clone());
        Some(event)
    }

    pub(crate) fn close_all(&self) {
        self.panels.lock().expect("panels lock").clear();
    }
}
