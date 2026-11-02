use std::collections::BTreeSet;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

pub mod catalog;
mod commands;
pub mod config;
pub mod fmt;
pub mod search;
mod tool;

use catalog::{Inputs, Plan, SECTION};
use config::{Config, NAMESPACE};

/// Session entry that records tools the model loaded, so a resumed session keeps them.
pub(crate) const LOAD_ENTRY: &str = "deferred-load";

#[derive(Default)]
pub(crate) struct Session {
    pub(crate) config: Config,
    pub(crate) sources: Vec<String>,
    pub(crate) loaded: BTreeSet<String>,
    pub(crate) removed: BTreeSet<String>,
    pub(crate) plan: Option<Plan>,
}

pub(crate) type Shared = Arc<Mutex<Session>>;

pub(crate) fn lock(state: &Shared) -> MutexGuard<'_, Session> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Deserialize)]
struct LoadEntry {
    #[serde(default)]
    tools: Vec<String>,
}

async fn project_trusted(host: &Host, snapshot: &Snapshot) -> bool {
    if snapshot.is_project_trusted {
        return true;
    }
    let answer = host.pi_call("ctx", "isProjectTrusted", Vec::new()).await;
    answer.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Layers: `<agentDir>/ext/deferred.json`, the `deferred` setting, then the trusted project file.
pub(crate) async fn load_config(host: &Host, snapshot: &Snapshot) -> (Config, Vec<String>) {
    let global_path = config::global_path(&host.agent_dir());
    let global = config::read_file(&global_path);
    let setting = host
        .settings::<Value>(NAMESPACE)
        .await
        .unwrap_or_else(|err| {
            pi_ext_sdk::warn!("settings.get {NAMESPACE} failed: {}", err.message());
            None
        });
    let project_path = config::project_path(&host.cwd());
    let project = match project_path.is_file() && project_trusted(host, snapshot).await {
        true => config::read_file(&project_path),
        false => None,
    };
    let (global_name, project_name) = (
        global_path.display().to_string(),
        project_path.display().to_string(),
    );
    let layers = [
        (global_name.as_str(), global.as_ref()),
        ("setting `deferred`", setting.as_ref()),
        (project_name.as_str(), project.as_ref()),
    ];
    let sources: Vec<String> = layers
        .iter()
        .filter(|(_, v)| v.is_some())
        .map(|(n, _)| (*n).to_owned())
        .collect();
    let sources = if sources.is_empty() {
        vec!["defaults".to_owned()]
    } else {
        sources
    };
    (Config::merge(layers), sources)
}

async fn session_start(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let (config, sources) = load_config(ev.host(), ev.snapshot()).await;
    let loaded: Vec<LoadEntry> = ev.host().entries_of(LOAD_ENTRY).await.unwrap_or_default();
    let mut session = lock(&state);
    *session = Session {
        config,
        sources,
        ..Session::default()
    };
    session
        .loaded
        .extend(loaded.into_iter().flat_map(|entry| entry.tools));
    Ok(EventReply::none())
}

async fn before_agent_start(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let host = ev.host();
    let options = ev
        .get("systemPromptOptions")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let tools = match (host.all_tools().await, host.active_tools().await) {
        (Ok(all), Ok(active)) => Some((all, active)),
        (Err(err), _) | (_, Err(err)) => {
            pi_ext_sdk::warn!("cannot read the tool loadout: {}", err.message());
            None
        }
    };
    let (all, active) = tools.clone().unwrap_or_default();
    let window = ev.snapshot().model.as_ref().and_then(|m| m.context_window);
    let plan = {
        let session = lock(&state);
        catalog::plan(&Inputs {
            config: &session.config,
            tools: &all,
            active: &active,
            options: &options,
            context_window: window,
            loaded: &session.loaded,
            removed: &session.removed,
        })
    };
    if tools.is_some() && plan.active != active {
        if let Err(err) = host.set_active_tools(&plan.active).await {
            pi_ext_sdk::warn!("tools.setActive failed: {}", err.message());
        }
    }
    let mut reply = EventReply::none().with_section(SECTION, fmt::index(&plan.items));
    if let Some(skills) = &plan.skills {
        reply = reply.with_patch(json!({ "systemPromptOptions": { "skills": skills } }));
    }
    if let Some(files) = &plan.context_files {
        reply = reply.with_patch(json!({ "systemPromptOptions": { "contextFiles": files } }));
    }
    for name in &plan.drop_sections {
        reply = reply.with_section(name, None);
    }
    let mut session = lock(&state);
    session.removed.extend(plan.newly_removed.iter().cloned());
    for name in plan.restored.iter().chain(&plan.adopted) {
        session.removed.remove(name);
    }
    session.loaded.extend(plan.adopted.iter().cloned());
    session.plan = Some(plan);
    Ok(reply)
}

pub fn extension() -> Extension {
    let state: Shared = Arc::default();
    let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state);
    Extension::new("deferred")
        .on("session_start", move |ev| session_start(s1.clone(), ev))
        .on("before_agent_start", move |ev| {
            before_agent_start(s2.clone(), ev)
        })
        .tool(tool::spec(), move |args, ctx| {
            tool::run(s3.clone(), args, ctx)
        })
        .command("deferred", commands::DESCRIPTION, move |args, ctx| {
            commands::run(s4.clone(), args, ctx)
        })
}

pub fn main() -> ExitCode {
    extension().run()
}
