use std::future::Future;

use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::Host;
use crate::ctx::{CommandCtx, EventCtx, ToolCtx};
use crate::error::Result;
use crate::handler;
use crate::registry::{Command, VirtualModel, put};
use crate::render::{EntryRender, MessageRender};
use crate::reply::{EventReply, ToolResult};
use crate::spec::{
    AutocompleteRequest, CommandSpec, FlagSpec, RouteRequest, ShortcutSpec, Suggestions, ToolSpec,
};

/// `register.*` and `virtualModel.*`: registrations after `initialize`, with their handlers.
impl Host {
    fn registry(&self) -> std::sync::RwLockWriteGuard<'_, crate::registry::Registry> {
        self.inner.registry.write().expect("registry lock")
    }

    pub async fn register_tool<A, R, F, Fut>(&self, spec: ToolSpec, run: F) -> Result<()>
    where
        A: DeserializeOwned,
        R: Into<ToolResult>,
        F: Fn(A, ToolCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
    {
        let params = {
            let mut registry = self.registry();
            let params = serde_json::to_value(registry.tool_spec(&spec))?;
            registry.put_tool(spec, handler::tool(run));
            params
        };
        self.call("register.tool", params).await.map(drop)
    }

    pub async fn register_command<F, Fut>(
        &self,
        name: &str,
        description: &str,
        run: F,
    ) -> Result<()>
    where
        F: Fn(String, CommandCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let spec = CommandSpec {
            name: name.to_owned(),
            description: Some(description.to_owned()),
            completions: false,
        };
        let params = serde_json::to_value(&spec)?;
        let run = handler::command(run);
        self.registry().put_command(Command {
            spec,
            run,
            complete: None,
        });
        self.call("register.command", params).await.map(drop)
    }

    pub async fn register_shortcut<F, Fut>(
        &self,
        key: &str,
        description: &str,
        run: F,
    ) -> Result<()>
    where
        F: Fn(CommandCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let spec = ShortcutSpec {
            shortcut: key.to_owned(),
            description: Some(description.to_owned()),
        };
        let params = serde_json::to_value(&spec)?;
        self.registry()
            .shortcuts
            .push((spec, handler::shortcut(run)));
        self.call("register.shortcut", params).await.map(drop)
    }

    pub async fn register_flag(&self, flag: FlagSpec) -> Result<()> {
        let params = serde_json::to_value(&flag)?;
        self.registry().flags.push(flag);
        self.call("register.flag", params).await.map(drop)
    }

    /// Adds an event handler and subscribes to the event when it is new.
    pub async fn on<R, F, Fut>(&self, event: &str, run: F) -> Result<()>
    where
        R: Into<EventReply>,
        F: Fn(EventCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
    {
        let is_new = {
            let mut registry = self.registry();
            let is_new = registry.event_handlers(event).is_empty();
            registry
                .events
                .push((event.to_owned(), handler::event(run)));
            is_new
        };
        if !is_new {
            return Ok(());
        }
        let params = json!({ "events": [event] });
        self.call("register.events", params).await.map(drop)
    }

    pub async fn register_autocomplete<F, Fut>(&self, triggers: &[&str], run: F) -> Result<()>
    where
        F: Fn(AutocompleteRequest, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<Suggestions>>> + Send + 'static,
    {
        let triggers: Vec<String> = triggers.iter().map(|t| (*t).to_owned()).collect();
        let params = json!({ "triggerCharacters": triggers });
        self.registry()
            .autocomplete
            .push((triggers, handler::with_host(run)));
        self.call("register.autocomplete", params).await.map(drop)
    }

    /// [`crate::Extension::message_renderer`] after `initialize`.
    pub async fn register_message_renderer<F, Fut>(
        &self,
        custom_type: &str,
        render: F,
    ) -> Result<()>
    where
        F: Fn(MessageRender, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<Vec<String>>>> + Send + 'static,
    {
        put(
            &mut self.registry().message_renderers,
            custom_type,
            handler::with_host(render),
        );
        let params = json!({ "customType": custom_type });
        self.call("register.messageRenderer", params)
            .await
            .map(drop)
    }

    /// [`crate::Extension::entry_renderer`] after `initialize`.
    pub async fn register_entry_renderer<F, Fut>(&self, custom_type: &str, render: F) -> Result<()>
    where
        F: Fn(EntryRender, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<Vec<String>>>> + Send + 'static,
    {
        put(
            &mut self.registry().entry_renderers,
            custom_type,
            handler::with_host(render),
        );
        let params = json!({ "customType": custom_type });
        self.call("register.entryRenderer", params).await.map(drop)
    }

    /// Registers a virtual model; pi sends `virtualModel.route` to `route` for each request.
    pub async fn register_virtual_model<F, Fut>(&self, definition: Value, route: F) -> Result<()>
    where
        F: Fn(RouteRequest, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let model = VirtualModel {
            definition: definition.clone(),
            route: handler::with_host(route),
        };
        self.registry().virtual_models.push(model);
        self.call("virtualModel.register", definition)
            .await
            .map(drop)
    }

    pub async fn unregister_virtual_model(&self, provider: &str, id: &str) -> Result<()> {
        self.registry()
            .virtual_models
            .retain(|model| !model.matches(provider, id));
        let params = json!({ "provider": provider, "id": id });
        self.call("virtualModel.unregister", params).await.map(drop)
    }
}
