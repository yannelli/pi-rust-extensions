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
pub mod render;
mod reply;
mod server;
mod snapshot;
mod spec;
pub mod style;
mod surface;
#[cfg(feature = "test-util")]
pub mod test_util;

pub use bus::Subscription;
pub use ctx::{CommandCtx, EventCtx, ToolCtx};
pub use error::{Error, Result, code};
pub use extension::Extension;
pub use host::{
    CustomMessage, DeliverAs, ExecOutput, ExecRequest, Host, InputOptions, LlmRequest, LlmResponse,
    NotifyType, Placement, ProviderAuth, ProviderUsage, UsageWindow,
};
pub use keys::Key;
pub use panel::{KeyInput, Panel, PanelBuilder, PanelEvent};
pub use protocol::PROTOCOL_VERSION;
pub use render::{
    EntryRender, MessageRender, SettingsScope, ToolCallRender, ToolRenderContext, ToolResultRender,
};
pub use reply::{EventReply, ToolResult, content, entry};
pub use snapshot::{ContextUsage, InitParams, Model, ScopedModel, Snapshot};
pub use spec::{
    AutocompleteRequest, CommandSpec, CompletionItem, FlagSpec, FlagType, OAuthSpec, RouteRequest,
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
