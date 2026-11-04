//! Agent configs as `contextEntries` of `resources_discover`, so pi lists them in the system
//! prompt or defers them to `tool_search` (pi-rust PR #226, `contextExposure` setting).

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use pi_ext_sdk::prelude::*;

use crate::agent::AgentDef;
use crate::app::App;
use crate::render;

/// The `initialize` feature of hosts that read `contextEntries`.
pub const FEATURE: &str = "contextEntries";
/// The kind of the entries; `contextExposure` uses it as the key.
pub const KIND: &str = "agent";
/// The parameter of the `subagent` tool that names the agent.
pub const AGENT_PARAM: &str = "agent";

/// Whether pi lists the entries of `resources_discover` itself.
pub fn host_supports(host: &Host) -> bool {
    host.init().is_some_and(|init| init.has_feature(FEATURE))
}

/// The entry of one agent; `None` without a description, which pi needs.
pub fn entry(agent: &AgentDef, tool: &str) -> Option<Value> {
    let description = agent.description.trim();
    if description.is_empty() {
        return None;
    }
    let mut entry = json!({
        "kind": KIND,
        "name": agent.name,
        "description": description,
        "usage": format!("Call the {tool} tool with {AGENT_PARAM} \"{}\".", agent.name),
    });
    if let Some(path) = agent.files.last() {
        entry["path"] = json!(path.display().to_string());
    }
    if let Some(exposure) = agent.exposure {
        entry["exposure"] = json!(exposure.as_str());
    }
    Some(entry)
}

/// The agent names of the last `resources_discover` answer; `None` before the first one.
#[derive(Clone, Default)]
pub struct Offered(Arc<Mutex<Option<BTreeSet<String>>>>);

impl Offered {
    fn set(&self, names: BTreeSet<String>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(names);
    }

    pub fn names(&self) -> Option<BTreeSet<String>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The `resources_discover` result: no entries on older hosts or with `advertise: false`.
pub fn discover(app: &App, host: &Host) -> EventReply {
    if !host_supports(host) || !app.config(host).advertise {
        return EventReply::none();
    }
    let tool = crate::tool::spec().name;
    let agents = app.agents(host);
    let entries: Vec<Value> = agents.iter().filter_map(|a| entry(a, &tool)).collect();
    let names = entries.iter().filter_map(|e| e["name"].as_str());
    app.offered.set(names.map(str::to_owned).collect());
    EventReply::result(json!({ "contextEntries": entries }))
}

/// The `subagents` section. pi lists the agents it got as entries, so the section lists
/// only the others: agents without a description and agents added since the discovery.
pub fn prompt_section(agents: &[AgentDef], offered: Option<&BTreeSet<String>>) -> String {
    let Some(offered) = offered else {
        return render::prompt_section(agents);
    };
    let rest: Vec<AgentDef> = agents
        .iter()
        .filter(|agent| !offered.contains(&agent.name))
        .cloned()
        .collect();
    let mut text = format!(
        "{}\n\nAgents for the `subagent` tool are context entries of kind `{KIND}`: they are listed in \
`<available_context>`, or `tool_search` finds them. `action: list` shows their tools and models.\n",
        render::PROMPT_HEADING
    );
    if !rest.is_empty() {
        text.push_str(&format!("\nOther agents:\n{}", render::agent_lines(&rest)));
    }
    text
}
