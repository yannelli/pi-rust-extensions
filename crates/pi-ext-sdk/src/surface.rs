use std::collections::HashMap;
use std::sync::Mutex;

use tokio::sync::{broadcast, mpsc};

use crate::keys::Key;
use crate::panel::{KeyInput, PanelEvent, PanelMessage};

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

/// The last `ui.resize` of one surface; `height` is the terminal rows when pi sent them.
#[derive(Debug, Clone, Copy, Default)]
struct Size {
    width: usize,
    height: Option<usize>,
}

pub(crate) struct Surfaces {
    panels: Mutex<HashMap<String, mpsc::UnboundedSender<PanelMessage>>>,
    sizes: Mutex<HashMap<String, Size>>,
    events: broadcast::Sender<SurfaceEvent>,
}

impl Surfaces {
    pub(crate) fn new() -> Self {
        Self {
            panels: Mutex::new(HashMap::new()),
            sizes: Mutex::new(HashMap::new()),
            events: broadcast::channel(64).0,
        }
    }

    pub(crate) fn add_panel(&self, surface: &str, tx: mpsc::UnboundedSender<PanelMessage>) {
        self.panels
            .lock()
            .expect("panels lock")
            .insert(surface.to_owned(), tx);
    }

    pub(crate) fn remove_panel(&self, surface: &str) {
        self.panels.lock().expect("panels lock").remove(surface);
        self.sizes.lock().expect("sizes lock").remove(surface);
    }

    fn panel(&self, surface: &str) -> Option<mpsc::UnboundedSender<PanelMessage>> {
        self.panels
            .lock()
            .expect("panels lock")
            .get(surface)
            .cloned()
    }

    fn size(&self, surface: &str) -> Option<Size> {
        self.sizes.lock().expect("sizes lock").get(surface).copied()
    }

    pub(crate) fn width(&self, surface: &str) -> Option<usize> {
        self.size(surface).map(|size| size.width)
    }

    pub(crate) fn height(&self, surface: &str) -> Option<usize> {
        self.size(surface)?.height
    }

    pub(crate) fn subscribe(&self) -> SurfaceEvents {
        SurfaceEvents {
            rx: self.events.subscribe(),
        }
    }

    /// Routes a `ui.resize`; a missing width or height keeps the last value.
    /// Returns the event for non-panel surfaces.
    pub(crate) fn resize(
        &self,
        surface: &str,
        width: Option<usize>,
        height: Option<usize>,
    ) -> Option<SurfaceEvent> {
        let mut sizes = self.sizes.lock().expect("sizes lock");
        let size = sizes.entry(surface.to_owned()).or_default();
        size.width = width.unwrap_or(size.width);
        size.height = height.or(size.height);
        let width = size.width;
        drop(sizes);
        if let Some(panel) = self.panel(surface) {
            let _ = panel.send((PanelEvent::Resize(width), None));
            return None;
        }
        let event = SurfaceEvent::Resize {
            surface: surface.to_owned(),
            width,
        };
        let _ = self.events.send(event.clone());
        Some(event)
    }

    pub(crate) fn key(&self, surface: &str, data: &str, id: Option<&str>) {
        if let Some(panel) = self.panel(surface) {
            let input = KeyInput {
                key: Key::parse(data),
                data: data.to_owned(),
                id: id.map(str::to_owned),
            };
            let _ = panel.send((PanelEvent::Key(input.key.clone()), Some(input)));
        }
    }

    /// Routes a `ui.closed`; returns the event for non-panel surfaces.
    pub(crate) fn closed(&self, surface: &str) -> Option<SurfaceEvent> {
        if self.panel(surface).is_some() {
            self.remove_panel(surface);
            return None;
        }
        self.sizes.lock().expect("sizes lock").remove(surface);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_size_clears_with_the_surface() {
        let surfaces = Surfaces::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        surfaces.add_panel("p", tx);
        assert_eq!(surfaces.resize("p", Some(80), Some(12)), None);
        assert_eq!(surfaces.resize("p", None, Some(20)), None);
        assert_eq!(rx.try_recv().unwrap().0, PanelEvent::Resize(80));
        assert_eq!(rx.try_recv().unwrap().0, PanelEvent::Resize(80));
        assert_eq!(
            (surfaces.width("p"), surfaces.height("p")),
            (Some(80), Some(20))
        );
        surfaces.closed("p");
        assert_eq!((surfaces.width("p"), surfaces.height("p")), (None, None));
    }

    #[test]
    fn widget_resize_without_height_keeps_the_event_shape() {
        let surfaces = Surfaces::new();
        let event = surfaces.resize("widget:a", Some(30), None);
        let expected = SurfaceEvent::Resize {
            surface: "widget:a".into(),
            width: 30,
        };
        assert_eq!(event, Some(expected));
        assert_eq!(surfaces.height("widget:a"), None);
        surfaces.closed("widget:a");
        assert_eq!(surfaces.width("widget:a"), None);
    }
}
