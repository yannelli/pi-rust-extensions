//! `/agents`: list, create, edit, set model and tools, reset, and delete agent definitions.

use pi_ext_sdk::prelude::*;

use crate::agent::AgentDef;
use crate::agents_edit::{create, save, set_model, set_tools};
use crate::app::App;
use crate::frontmatter::FmValue;

const THINKING: [&str; 8] = [
    "inherit", "off", "minimal", "low", "medium", "high", "xhigh", "max",
];
const NEW: &str = "+ New agent";
const RUNS: &str = "Runs (running and recent)";

pub async fn open(app: App, host: Host) -> Result<()> {
    loop {
        let agents = app.agents(&host);
        let mut options: Vec<String> = agents.iter().map(agent_option).collect();
        options.extend([NEW.to_owned(), RUNS.to_owned()]);
        let Some(choice) = host.select("Subagents", &options).await? else {
            return Ok(());
        };
        if choice == NEW {
            if let Some(name) = create(&app, &host, &agents).await? {
                agent_menu(&app, &host, &name).await?;
            }
        } else if choice == RUNS {
            return crate::runs_panel::open(app, host).await;
        } else if let Some(index) = options.iter().position(|o| *o == choice) {
            let name = agents[index].name.clone();
            agent_menu(&app, &host, &name).await?;
        }
    }
}

fn agent_option(agent: &AgentDef) -> String {
    let origin = match (agent.builtin, agent.overridden()) {
        (true, true) => format!("built-in, {} override", agent.source.label()),
        (true, false) => "built-in".to_owned(),
        (false, _) => agent.source.label().to_owned(),
    };
    format!(
        "{} ({origin}) \u{2014} {}",
        agent.name,
        crate::runs::first_line(&agent.description, 70)
    )
}

async fn agent_menu(app: &App, host: &Host, name: &str) -> Result<()> {
    loop {
        let Some(agent) = app.agents(host).into_iter().find(|a| a.name == name) else {
            return Ok(());
        };
        let mut actions = vec![
            format!("Edit prompt ({} chars)", agent.prompt.len()),
            format!(
                "Description: {}",
                crate::runs::first_line(&agent.description, 50)
            ),
            format!(
                "Model: {}",
                agent.model.as_deref().unwrap_or("inherit (session model)")
            ),
            format!(
                "Thinking: {}",
                agent.thinking.as_deref().unwrap_or("inherit")
            ),
            format!("Tools: {}", agent.tools_label()),
            format!(
                "Max turns: {}",
                agent.max_turns.map_or("none".into(), |t| t.to_string())
            ),
        ];
        if agent.overridden() {
            actions.push("Reset to default".into());
        }
        if !agent.builtin {
            actions.push("Delete".into());
        }
        let title = format!("Agent {} ({})", agent.name, agent.source.label());
        let Some(action) = host.select(&title, &actions).await? else {
            return Ok(());
        };
        let verb = action.split([' ', ':']).next().unwrap_or("");
        match verb {
            "Edit" => {
                if let Some(prompt) = host
                    .editor(&format!("System prompt of {name}"), Some(&agent.prompt))
                    .await?
                {
                    save(app, host, &agent, |fm| fm.body = prompt.trim().to_owned()).await?;
                }
            }
            "Description" => {
                if let Some(text) = host
                    .input(&format!("Description of {name}"), Some(&agent.description))
                    .await?
                {
                    let text = text.trim().to_owned();
                    if !text.is_empty() {
                        save(app, host, &agent, |fm| {
                            fm.set("description", Some(FmValue::Text(text)))
                        })
                        .await?;
                    }
                }
            }
            "Model" => set_model(app, host, &agent).await?,
            "Thinking" => {
                if let Some(level) = host
                    .select(&format!("Thinking level of {name}"), &THINKING)
                    .await?
                {
                    save(app, host, &agent, |fm| {
                        fm.set("thinking", Some(FmValue::Text(level)))
                    })
                    .await?;
                }
            }
            "Tools" => set_tools(app, host, &agent).await?,
            "Max" => {
                let current = agent.max_turns.map(|t| t.to_string()).unwrap_or_default();
                if let Some(text) = host
                    .input("Max turns (empty for no limit)", Some(&current))
                    .await?
                {
                    let value = text.trim().parse::<u32>().ok().filter(|t| *t > 0);
                    save(app, host, &agent, |fm| {
                        fm.set("maxTurns", value.map(|t| FmValue::Text(t.to_string())))
                    })
                    .await?;
                }
            }
            "Reset" | "Delete" => {
                let what = if verb == "Reset" { "Reset" } else { "Delete" };
                let files: Vec<String> = agent
                    .files
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect();
                if host
                    .confirm(
                        &format!("{what} {name}?"),
                        &format!("Removes {}", files.join(", ")),
                    )
                    .await?
                {
                    for path in &agent.files {
                        std::fs::remove_file(path)?;
                    }
                    host.notify_info(format!("{what} {name}: removed {} file(s)", files.len()))
                        .await?;
                    if verb == "Delete" {
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
    }
}
