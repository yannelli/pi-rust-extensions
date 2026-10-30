#![doc = include_str!("../README.md")]

mod bus;
mod ctx;
mod dispatch;
mod error;
pub mod event_types;
mod extension;
mod handler;
mod host;
pub mod json;
pub mod keys;
#[doc(hidden)]
pub mod log;
mod panel;
pub mod paths;
mod peer;
mod protocol;
mod registry;
mod reply;
mod server;
mod snapshot;
mod spec;
pub mod style;
mod surface;

pub use bus::Subscription;
pub use ctx::{CommandCtx, EventCtx, ToolCtx};
pub use error::{Error, Result, code};
pub use extension::Extension;
pub use host::{
    CustomMessage, DeliverAs, ExecOutput, ExecRequest, Host, LlmRequest, LlmResponse, NotifyType,
    Placement, ProviderAuth,
};
pub use keys::Key;
pub use panel::{Panel, PanelBuilder, PanelEvent};
pub use protocol::PROTOCOL_VERSION;
pub use reply::{EventReply, ToolResult, content};
pub use snapshot::{InitParams, Model, Snapshot};
pub use spec::{
    AutocompleteRequest, CommandSpec, CompletionItem, FlagSpec, FlagType, RouteRequest,
    ShortcutSpec, Suggestions, ToolSpec,
};
pub use surface::{SurfaceEvent, SurfaceEvents};
pub use tokio_util::sync::CancellationToken;

pub use serde_json;
pub use tokio;

/// Common imports for extension crates.
pub mod prelude {
    pub use crate::{
        CommandCtx, CustomMessage, DeliverAs, Error, EventCtx, EventReply, Extension, FlagSpec,
        Host, Key, Panel, PanelEvent, Placement, Result, Snapshot, SurfaceEvent, ToolCtx,
        ToolResult, ToolSpec, bail, content, style,
    };
    pub use serde_json::{Value, json};
}
