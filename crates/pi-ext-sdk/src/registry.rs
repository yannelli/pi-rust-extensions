use serde_json::{Map, Value, json};

use crate::handler::{
    AutocompleteFn, CommandFn, CompleteFn, EventFn, FooterFn, HostFn, PrepareFn, RenderFn, RouteFn,
    ShortcutFn, SurfaceFn, ToolFn,
};
use crate::protocol::PROTOCOL_VERSION;
use crate::render::{EntryRender, MessageRender, ToolCallRender, ToolResultRender};
use crate::spec::{CommandSpec, FlagSpec, ShortcutSpec, ToolSpec};

pub(crate) struct Command {
    pub(crate) spec: CommandSpec,
    pub(crate) run: CommandFn,
    pub(crate) complete: Option<CompleteFn>,
}

pub(crate) struct VirtualModel {
    pub(crate) definition: Value,
    pub(crate) route: RouteFn,
}

impl VirtualModel {
    pub(crate) fn matches(&self, provider: &str, id: &str) -> bool {
        self.definition.get("provider").and_then(Value::as_str) == Some(provider)
            && self.definition.get("id").and_then(Value::as_str) == Some(id)
    }
}

/// Everything the extension registered, shared between the builder, `Host`, and dispatch.
#[derive(Default)]
pub(crate) struct Registry {
    pub(crate) tools: Vec<(ToolSpec, ToolFn)>,
    pub(crate) commands: Vec<Command>,
    pub(crate) shortcuts: Vec<(ShortcutSpec, ShortcutFn)>,
    pub(crate) flags: Vec<FlagSpec>,
    pub(crate) events: Vec<(String, EventFn)>,
    pub(crate) providers: Vec<(String, Value)>,
    pub(crate) virtual_models: Vec<VirtualModel>,
    pub(crate) autocomplete: Vec<(Vec<String>, AutocompleteFn)>,
    pub(crate) on_start: Vec<HostFn>,
    pub(crate) on_shutdown: Vec<HostFn>,
    pub(crate) on_surface: Vec<SurfaceFn>,
    pub(crate) on_footer: Vec<FooterFn>,
    pub(crate) prepare: Vec<(String, PrepareFn)>,
    pub(crate) call_renderers: Vec<(String, RenderFn<ToolCallRender>)>,
    pub(crate) result_renderers: Vec<(String, RenderFn<ToolResultRender>)>,
    pub(crate) message_renderers: Vec<(String, RenderFn<MessageRender>)>,
    pub(crate) entry_renderers: Vec<(String, RenderFn<EntryRender>)>,
}

/// The entry of `key` in a `(key, value)` list; a later `put` replaces it.
pub(crate) fn find<T: Clone>(list: &[(String, T)], key: &str) -> Option<T> {
    list.iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.clone())
}

pub(crate) fn put<T>(list: &mut Vec<(String, T)>, key: &str, value: T) {
    list.retain(|(name, _)| name != key);
    list.push((key.to_owned(), value));
}

fn has_key<T>(list: &[(String, T)], key: &str) -> bool {
    list.iter().any(|(name, _)| name == key)
}

fn keys<T>(list: &[(String, T)]) -> Vec<&str> {
    list.iter().map(|(key, _)| key.as_str()).collect()
}

impl Registry {
    /// `spec` with the `prepareArguments`, `renderCall`, and `renderResult` flags of its handlers.
    pub(crate) fn tool_spec(&self, spec: &ToolSpec) -> ToolSpec {
        let mut spec = spec.clone();
        spec.prepare_arguments |= has_key(&self.prepare, &spec.name);
        spec.render_call |= has_key(&self.call_renderers, &spec.name);
        spec.render_result |= has_key(&self.result_renderers, &spec.name);
        spec
    }

    pub(crate) fn put_tool(&mut self, spec: ToolSpec, run: ToolFn) {
        self.tools
            .retain(|(existing, _)| existing.name != spec.name);
        self.tools.push((spec, run));
    }

    pub(crate) fn put_command(&mut self, command: Command) {
        self.commands.retain(|c| c.spec.name != command.spec.name);
        self.commands.push(command);
    }

    pub(crate) fn tool(&self, name: &str) -> Option<ToolFn> {
        let found = self.tools.iter().find(|(spec, _)| spec.name == name);
        found.map(|(_, run)| run.clone())
    }

    pub(crate) fn command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.spec.name == name)
    }

    pub(crate) fn shortcut(&self, key: &str) -> Option<ShortcutFn> {
        let found = self.shortcuts.iter().find(|(spec, _)| spec.shortcut == key);
        found.map(|(_, run)| run.clone())
    }

    pub(crate) fn event_handlers(&self, name: &str) -> Vec<EventFn> {
        let matching = self.events.iter().filter(|(event, _)| event == name);
        matching.map(|(_, run)| run.clone()).collect()
    }

    pub(crate) fn route(&self, provider: &str, id: &str) -> Option<RouteFn> {
        let found = self.virtual_models.iter().find(|m| m.matches(provider, id));
        found.map(|m| m.route.clone())
    }

    pub(crate) fn event_names(&self) -> Vec<String> {
        let mut names: Vec<String> = Vec::new();
        for (name, _) in &self.events {
            if !names.contains(name) {
                names.push(name.clone());
            }
        }
        if !self.on_start.is_empty() && !names.iter().any(|name| name == "session_start") {
            names.push("session_start".into());
        }
        if !self.on_footer.is_empty() && !names.iter().any(|name| name == "footer_changed") {
            names.push("footer_changed".into());
        }
        names
    }

    pub(crate) fn initialize_result(&self) -> Value {
        let mut result = Map::new();
        result.insert("protocolVersion".into(), json!(PROTOCOL_VERSION));
        let tools: Vec<ToolSpec> = self
            .tools
            .iter()
            .map(|(spec, _)| self.tool_spec(spec))
            .collect();
        let commands: Vec<&CommandSpec> = self.commands.iter().map(|c| &c.spec).collect();
        let shortcuts: Vec<&ShortcutSpec> = self.shortcuts.iter().map(|(s, _)| s).collect();
        let providers: Vec<Value> = self
            .providers
            .iter()
            .map(|(name, config)| json!({ "name": name, "config": config }))
            .collect();
        let models: Vec<&Value> = self.virtual_models.iter().map(|m| &m.definition).collect();
        let autocomplete: Vec<Value> = self
            .autocomplete
            .iter()
            .map(|(triggers, _)| json!({ "triggerCharacters": triggers }))
            .collect();
        let fields = [
            ("tools", json!(tools)),
            ("commands", json!(commands)),
            ("shortcuts", json!(shortcuts)),
            ("flags", json!(self.flags)),
            ("events", json!(self.event_names())),
            ("providers", json!(providers)),
            ("virtualModels", json!(models)),
            ("autocomplete", json!(autocomplete)),
            ("messageRenderers", json!(keys(&self.message_renderers))),
            ("entryRenderers", json!(keys(&self.entry_renderers))),
        ];
        for (key, value) in fields {
            if value.as_array().is_some_and(|items| !items.is_empty()) {
                result.insert(key.into(), value);
            }
        }
        Value::Object(result)
    }
}
