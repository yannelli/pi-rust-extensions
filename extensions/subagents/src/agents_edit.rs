//! Edits behind `/agents`: model and tools pickers, scope choice, saving, and new agents.

use std::path::{Path, PathBuf};

use pi_ext_sdk::prelude::*;

use crate::agent::{AgentDef, Source, new_agent_file};
use crate::app::App;
use crate::frontmatter::FmValue;
use crate::registry::{self, sanitize_name};

pub async fn set_model(app: &App, host: &Host, agent: &AgentDef) -> Result<()> {
    let mut options = vec!["inherit (session model and thinking)".to_owned()];
    let models = host
        .pi_call("ctx.modelRegistry", "getAvailable", Vec::new())
        .await
        .unwrap_or_default();
    for model in models.as_array().into_iter().flatten() {
        if let (Some(provider), Some(id)) = (model["provider"].as_str(), model["id"].as_str()) {
            options.push(format!("{provider}/{id}"));
        }
    }
    options.push("Enter provider/id\u{2026}".into());
    let Some(choice) = host
        .select(&format!("Model of {}", agent.name), &options)
        .await?
    else {
        return Ok(());
    };
    let model = if choice.starts_with("inherit") {
        "inherit".to_owned()
    } else if choice.starts_with("Enter") {
        match host
            .input("Model (provider/id)", agent.model.as_deref())
            .await?
        {
            Some(text) if !text.trim().is_empty() => text.trim().to_owned(),
            _ => return Ok(()),
        }
    } else {
        choice
    };
    save(app, host, agent, |fm| {
        fm.set("model", Some(FmValue::Text(model)))
    })
    .await
}

pub async fn set_tools(app: &App, host: &Host, agent: &AgentDef) -> Result<()> {
    let available: Vec<String> = host
        .all_tools()
        .await
        .unwrap_or_default()
        .iter()
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .filter(|n| n != "subagent")
        .collect();
    let current = agent
        .tools
        .as_ref()
        .map_or("all".to_owned(), |t| t.join(", "));
    let title = format!(
        "Tools of {} (comma-separated, `all` for every tool). Available: {}",
        agent.name,
        available.join(", ")
    );
    let Some(text) = host.editor(&title, Some(&current)).await? else {
        return Ok(());
    };
    let names: Vec<String> = text
        .split([',', '\n'])
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
        .collect();
    let value = if names.iter().any(|n| n == "all" || n == "*") {
        FmValue::Text("all".into())
    } else {
        FmValue::List(names)
    };
    save(app, host, agent, |fm| fm.set("tools", Some(value))).await
}

/// Writes to the agent's highest file scope, or asks for a scope when it has no file.
pub async fn save(
    app: &App,
    host: &Host,
    agent: &AgentDef,
    edit: impl FnOnce(&mut crate::frontmatter::Frontmatter),
) -> Result<()> {
    let layers = app.layers(host);
    let dir = match agent.source {
        Source::Builtin => match choose_scope(host, &layers).await? {
            Some(dir) => dir,
            None => return Ok(()),
        },
        source => match layers.dir(source) {
            Some(dir) => dir.to_owned(),
            None => return Ok(()),
        },
    };
    let path = registry::edit_file(&dir, &agent.name, edit)?;
    host.notify_info(format!("Saved {}", path.display())).await
}

pub async fn choose_scope(host: &Host, layers: &registry::Layers) -> Result<Option<PathBuf>> {
    let global = format!("Global ({})", layers.global.display());
    let mut options = vec![global.clone()];
    if let Some(project) = &layers.project {
        options.push(format!("Project ({})", project.display()));
    }
    let choice = host.select("Save to", &options).await?;
    Ok(choice.map(|c| {
        if c == global {
            layers.global.clone()
        } else {
            layers.project.clone().unwrap_or_default()
        }
    }))
}

pub async fn create(app: &App, host: &Host, agents: &[AgentDef]) -> Result<Option<String>> {
    let Some(name) = host.input("Agent name", Some("reviewer")).await? else {
        return Ok(None);
    };
    let name = sanitize_name(&name);
    if name.is_empty() || agents.iter().any(|a| a.name == name) {
        host.notify_warning(format!("Agent name \"{name}\" is empty or taken"))
            .await?;
        return Ok(None);
    }
    let description = host
        .input("Description (when to use it)", None)
        .await?
        .unwrap_or_default();
    let Some(dir) = choose_scope(host, &app.layers(host)).await? else {
        return Ok(None);
    };
    let template = format!(
        "You are the {name} subagent. Describe its job, method, and the shape of its final answer."
    );
    let prompt = host
        .editor(&format!("System prompt of {name}"), Some(&template))
        .await?
        .unwrap_or(template);
    let file = new_agent_file(&name, description.trim(), prompt.trim());
    write_new(&dir, &name, &file.render())?;
    host.notify_info(format!("Created agent {name}")).await?;
    Ok(Some(name))
}

fn write_new(dir: &Path, name: &str, text: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(registry::file_for(dir, name), text)
}
