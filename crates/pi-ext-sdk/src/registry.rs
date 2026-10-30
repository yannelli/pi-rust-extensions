use serde_json::{Map, Value, json};

use crate::handler::{
    AutocompleteFn, CommandFn, CompleteFn, EventFn, HostFn, RouteFn, ShortcutFn, SurfaceFn, ToolFn,
};
use crate::protocol::PROTOCOL_VERSION;
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
}

impl Registry {
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
        names
    }

    pub(crate) fn initialize_result(&self) -> Value {
        let mut result = Map::new();
        result.insert("protocolVersion".into(), json!(PROTOCOL_VERSION));
        let tools: Vec<&ToolSpec> = self.tools.iter().map(|(spec, _)| spec).collect();
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
        ];
        for (key, value) in fields {
            if value.as_array().is_some_and(|items| !items.is_empty()) {
                result.insert(key.into(), value);
            }
        }
        Value::Object(result)
    }
}
