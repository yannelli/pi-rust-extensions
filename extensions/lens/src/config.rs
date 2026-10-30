use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pi_ext_sdk::{Error, Host, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Config {
    pub enabled: bool,
    pub context: bool,
    pub read_guard: bool,
    pub auto_detect: bool,
    pub timeout_ms: u64,
    pub tools: BTreeMap<String, Toggle>,
    pub languages: BTreeMap<String, Toggle>,
    pub commands: Vec<ToolCommand>,
    pub lsp: LspConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            context: true,
            read_guard: true,
            auto_detect: true,
            timeout_ms: 30_000,
            tools: BTreeMap::new(),
            languages: BTreeMap::new(),
            commands: Vec::new(),
            lsp: LspConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub struct Toggle {
    pub enabled: bool,
}

impl Default for Toggle {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolCommand {
    pub name: String,
    pub language: String,
    pub stage: Stage,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Format,
    Lint,
    Types,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct LspConfig {
    pub servers: Vec<LspServer>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LspServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub languages: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

pub fn read(path: &Path) -> Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error.into()),
    }
}

pub fn paths(agent_dir: &Path, cwd: &Path) -> [PathBuf; 2] {
    [
        agent_dir.join("ext/lens.json"),
        cwd.join(".pi/ext/lens.json"),
    ]
}

pub fn load(agent_dir: &Path, cwd: &Path, trusted: bool) -> Result<(Config, bool)> {
    let [global, project] = paths(agent_dir, cwd);
    let mut value = serde_json::to_value(Config::default())?;
    if let Some(path) = std::env::var_os("PI_LENS_CONFIG_PATH") {
        pi_ext_sdk::json::merge_patch(&mut value, &read(Path::new(&path))?);
    }
    pi_ext_sdk::json::merge_patch(&mut value, &read(&global)?);
    let legacy = cwd.ancestors().find_map(|dir| {
        let path = dir.join(".pi-lens.json");
        path.exists().then_some(path)
    });
    let mut ignored = false;
    for path in legacy.into_iter().chain([project]) {
        let mut project = read(&path)?;
        if !trusted {
            for key in ["commands", "lsp"] {
                if project.get(key).is_some() {
                    ignored = true;
                    project
                        .as_object_mut()
                        .ok_or_else(|| Error::msg("Lens config must be an object"))?
                        .remove(key);
                }
            }
        }
        pi_ext_sdk::json::merge_patch(&mut value, &project);
    }
    let config: Config = serde_json::from_value(value)?;
    config.validate()?;
    Ok((config, ignored))
}

impl Config {
    pub fn validate(&self) -> Result<()> {
        if !(1..=300_000).contains(&self.timeout_ms) {
            return Err(Error::msg("Lens timeoutMs must be between 1 and 300000"));
        }
        for command in &self.commands {
            if command.name.is_empty() || command.language.is_empty() || command.command.is_empty()
            {
                return Err(Error::msg("Lens commands need name, language and command"));
            }
        }
        if self
            .lsp
            .servers
            .iter()
            .any(|server| server.command.is_empty() || server.languages.is_empty())
        {
            return Err(Error::msg("Lens LSP servers need command and languages"));
        }
        Ok(())
    }

    pub fn tool_enabled(&self, name: &str) -> bool {
        self.tools.get(name).is_none_or(|tool| tool.enabled)
    }

    pub fn language_enabled(&self, language: &str) -> bool {
        self.languages.get(language).is_none_or(|item| item.enabled)
    }
}

pub async fn trusted(host: &Host) -> bool {
    if let Some(value) = host
        .snapshot()
        .and_then(|s| s.extra.get("isProjectTrusted").and_then(Value::as_bool))
    {
        return value;
    }
    host.pi_call("ctx", "isProjectTrusted", vec![])
        .await
        .ok()
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

pub fn save(path: &Path, value: &Value) -> Result<()> {
    let config: Config = serde_json::from_value(value.clone())?;
    config.validate()?;
    std::fs::create_dir_all(
        path.parent()
            .ok_or_else(|| Error::msg("Invalid config path"))?,
    )?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(temp, path)?;
    Ok(())
}
