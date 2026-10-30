//! `/goal-settings`: a select menu that writes keys to the global or project config file.

use pi_ext_sdk::prelude::*;

use crate::app::App;
use crate::config::{self, Scope, Settings};

const BOOLEANS: [(&str, &str); 7] = [
    ("disableTasks", "Task tree tools off"),
    ("disableContracts", "Verification contracts off"),
    ("disableAuditor", "Approve completion without the auditor"),
    ("strictExecutionContract", "Allow wait continuations"),
    ("showAutonomousRuns", "Show the run allowance in the prompt"),
    ("hideUnfocusedPrompt", "Hide the unfocused-goal prompt"),
    ("hideUnfocusedBanner", "Hide the unfocused-goal status"),
];

const TEXTS: [(&str, &str); 5] = [
    (
        "maxAutonomousRuns",
        "Autonomous runs per start/resume (empty = unlimited, 0 = off)",
    ),
    (
        "model",
        "Auditor model as provider/id (empty = current model)",
    ),
    ("thinkingLevel", "Auditor thinking level (empty = default)"),
    ("objectiveMaxChars", "Objective length limit (empty = none)"),
    (
        "goalsRoot",
        "Goal file directory relative to the project (empty = agent state dir)",
    ),
];

fn bool_value(settings: &Settings, key: &str) -> bool {
    match key {
        "disableTasks" => settings.disable_tasks,
        "disableContracts" => settings.disable_contracts,
        "disableAuditor" => settings.disable_auditor,
        "strictExecutionContract" => settings.strict_execution_contract,
        "showAutonomousRuns" => settings.show_autonomous_runs,
        "hideUnfocusedPrompt" => settings.hide_unfocused_prompt,
        _ => settings.hide_unfocused_banner,
    }
}

fn text_value(settings: &Settings, key: &str) -> String {
    let value = match key {
        "maxAutonomousRuns" => settings.max_autonomous_runs.map(|n| n.to_string()),
        "model" => settings.auditor_model(),
        "thinkingLevel" => settings.thinking_level.clone(),
        "objectiveMaxChars" => settings.objective_max_chars.map(|n| n.to_string()),
        _ => settings.goals_root.clone(),
    };
    value.unwrap_or_else(|| "default".into())
}

/// Parses menu input for `key`: `None` removes the key.
pub fn parse_text(key: &str, input: &str) -> std::result::Result<Option<Value>, String> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    match key {
        "maxAutonomousRuns" | "objectiveMaxChars" => input
            .parse::<u64>()
            .map(|n| Some(json!(n)))
            .map_err(|_| format!("{key} must be a whole number")),
        _ => Ok(Some(json!(input))),
    }
}

pub async fn run(app: &App, host: &Host) -> Result<()> {
    let mut scope = Scope::Project;
    loop {
        let settings = app.lock().settings.clone();
        let scope_label = match scope {
            Scope::Global => "Scope: global (~/.pi/agent/ext/goal.json)",
            Scope::Project => "Scope: project (.pi/ext/goal.json)",
        };
        let mut items = vec![scope_label.to_owned()];
        items.extend(BOOLEANS.iter().map(|(key, help)| {
            let state = if bool_value(&settings, key) {
                "on"
            } else {
                "off"
            };
            format!("{key}: {state} — {help}")
        }));
        items.extend(
            TEXTS
                .iter()
                .map(|(key, _)| format!("{key}: {}", text_value(&settings, key))),
        );
        items.push("Done".into());
        let Some(choice) = host.select("Goal settings", &items).await? else {
            return Ok(());
        };
        if choice == "Done" {
            return Ok(());
        }
        if choice == scope_label {
            scope = if scope == Scope::Global {
                Scope::Project
            } else {
                Scope::Global
            };
            continue;
        }
        let key = choice.split(':').next().unwrap_or_default().to_owned();
        let value = if BOOLEANS.iter().any(|(k, _)| *k == key) {
            Some(json!(!bool_value(&settings, &key)))
        } else {
            let help = TEXTS
                .iter()
                .find(|(k, _)| *k == key)
                .map_or("", |(_, h)| *h);
            let Some(input) = host.input(help, Some(&text_value(&settings, &key))).await? else {
                continue;
            };
            match parse_text(&key, &input) {
                Ok(value) => value,
                Err(err) => {
                    host.notify_warning(err).await?;
                    continue;
                }
            }
        };
        let path = config::config_path(scope, &host.agent_dir(), &host.cwd());
        if let Err(err) = config::write_key(&path, &key, value) {
            host.notify_error(format!("cannot write {}: {err}", path.display()))
                .await?;
            continue;
        }
        crate::events::on_session(app, host).await?;
    }
}
