use std::collections::BTreeSet;
use std::process::ExitCode;
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::prelude::*;
use serde::Deserialize;

pub mod catalog;
mod commands;
pub mod config;
pub mod fmt;
pub mod measure;
pub mod repo;
pub mod search;
mod tool;
pub mod ui;

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
    /// `systemPromptOptions` of the last prompt, for the panel when pi cannot answer.
    pub(crate) options: Option<Value>,
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

pub(crate) async fn project_trusted(host: &Host, snapshot: &Snapshot) -> bool {
    if snapshot.is_project_trusted {
        return true;
    }
    let answer = host.pi_call("ctx", "isProjectTrusted", Vec::new()).await;
    answer.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Layers: `<agentDir>/ext/deferred.json`, the `deferred` setting, the rules of the cwd's
/// repo, then the project setting and the trusted project file.
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
    let cwd = host.cwd();
    let project_path = config::project_path(&cwd);
    let settings_path = host.project_file("settings.json");
    let trusted = match project_path.is_file() || settings_path.is_file() {
        true => project_trusted(host, snapshot).await,
        false => false,
    };
    let project = trusted.then(|| config::read_file(&project_path)).flatten();
    let project_setting = trusted
        .then(|| config::read_file(&settings_path))
        .flatten()
        .and_then(|v| v.get(NAMESPACE).cloned());
    let repo = repo::detect(&cwd).await;
    let (global_name, project_name) = (
        global_path.display().to_string(),
        project_path.display().to_string(),
    );
    let config = Config::load(&config::Layers {
        global_file: global.as_ref().map(|v| (global_name.as_str(), v)),
        setting: setting.as_ref(),
        project_setting: project_setting.as_ref(),
        project_file: project.as_ref().map(|v| (project_name.as_str(), v)),
        repo: repo.as_deref(),
    });
    let layers = [
        (global_name, global.is_some()),
        (config::SETTING_SOURCE.to_owned(), setting.is_some()),
        (project_name, project.is_some()),
    ];
    let mut sources: Vec<String> = layers
        .into_iter()
        .filter(|(_, present)| *present)
        .map(|(name, _)| name)
        .collect();
    if sources.is_empty() {
        sources.push("defaults".to_owned());
    }
    (config, sources)
}

/// `off` skills: `/skill:name` is blocked too. Skills pi pins as user-only stay runnable.
async fn input(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let text = ev.get("text").and_then(Value::as_str).unwrap_or_default();
    let Some(rest) = text.strip_prefix("/skill:") else {
        return Ok(EventReply::none());
    };
    let name = rest.split_whitespace().next().unwrap_or_default();
    let blocked = {
        let session = lock(&state);
        let pinned = session
            .options
            .as_ref()
            .and_then(|o| o.get("skills")?.as_array())
            .into_iter()
            .flatten()
            .any(|s| {
                s.get("name").and_then(Value::as_str) == Some(name)
                    && s.get("disableModelInvocation").and_then(Value::as_bool) == Some(true)
            });
        let rule = session.config.rule(config::Kind::Skill, name);
        rule.filter(|r| r.mode == config::Mode::Off && !pinned)
            .map(|r| r.pattern.clone())
    };
    let Some(pattern) = blocked else {
        return Ok(EventReply::none());
    };
    let message = format!(
        "The skill {name} is off (rule `{pattern}`). Set it to `user` to run it with /skill:{name}."
    );
    if let Err(err) = ev.host().notify_warning(message).await {
        pi_ext_sdk::warn!("ui.notify failed: {}", err.message());
    }
    Ok(EventReply::handled())
}

/// `off` tools cannot run, also when pi's `tool_search` or codemode reaches them.
async fn tool_call(state: Shared, ev: EventCtx) -> Result<EventReply> {
    let name = ev
        .get("toolName")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if name == catalog::TOOL {
        return Ok(EventReply::none());
    }
    let session = lock(&state);
    Ok(match session.config.rule(config::Kind::Tool, name) {
        Some(rule) if rule.mode == config::Mode::Off => EventReply::block(format!(
            "The tool {name} is turned off (deferred rule `{}`).",
            rule.pattern
        )),
        _ => EventReply::none(),
    })
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
    session.options = Some(options);
    Ok(reply)
}

pub fn extension() -> Extension {
    let state: Shared = Arc::default();
    let (s1, s2, s3, s4) = (state.clone(), state.clone(), state.clone(), state.clone());
    let (s5, s6) = (state.clone(), state);
    Extension::new("deferred")
        .on("session_start", move |ev| session_start(s1.clone(), ev))
        .on("before_agent_start", move |ev| {
            before_agent_start(s2.clone(), ev)
        })
        .on("input", move |ev| input(s5.clone(), ev))
        .on("tool_call", move |ev| tool_call(s6.clone(), ev))
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
