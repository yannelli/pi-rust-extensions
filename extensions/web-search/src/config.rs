//! Config files: legacy `web-search.json`, global `<agentDir>/ext/web-search.json`, and the
//! trusted project `<cwd>/.pi/ext/web-search.json`, merged per key in that order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use pi_ext_sdk::json::merge_patch;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const NAME: &str = "web-search";
pub const LEGACY_ENV: &str = "PI_WEB_SEARCH_CONFIG";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    Exa,
    Firecrawl,
    Brave,
    Tavily,
}

impl Api {
    /// Order of the `auto` fallback.
    pub const ALL: [Api; 4] = [Api::Exa, Api::Tavily, Api::Brave, Api::Firecrawl];

    pub fn id(self) -> &'static str {
        match self {
            Api::Exa => "exa",
            Api::Firecrawl => "firecrawl",
            Api::Brave => "brave",
            Api::Tavily => "tavily",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Api::Exa => "Exa",
            Api::Firecrawl => "Firecrawl",
            Api::Brave => "Brave",
            Api::Tavily => "Tavily",
        }
    }

    pub fn env(self) -> &'static str {
        match self {
            Api::Exa => "EXA_API_KEY",
            Api::Firecrawl => "FIRECRAWL_API_KEY",
            Api::Brave => "BRAVE_API_KEY",
            Api::Tavily => "TAVILY_API_KEY",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Api::Exa => "https://api.exa.ai",
            Api::Firecrawl => "https://api.firecrawl.dev",
            Api::Brave => "https://api.search.brave.com",
            Api::Tavily => "https://api.tavily.com",
        }
    }

    /// Brave has no page fetch endpoint.
    pub fn fetches_urls(self) -> bool {
        self != Api::Brave
    }

    pub fn parse(id: &str) -> Option<Api> {
        Api::ALL.into_iter().find(|api| api.id() == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Auto,
    Native,
    Api(Api),
}

impl Choice {
    pub fn parse(id: &str) -> Option<Choice> {
        match id {
            "auto" => Some(Choice::Auto),
            "native" => Some(Choice::Native),
            other => Api::parse(other).map(Choice::Api),
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Choice::Auto => "auto",
            Choice::Native => "native",
            Choice::Api(api) => api.id(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderSettings {
    pub base_url: Option<String>,
    pub max_results: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub default_provider: Option<String>,
    pub tools: BTreeMap<String, String>,
    pub search_model: Option<String>,
    pub providers: BTreeMap<String, ProviderSettings>,
}

/// The upstream `{ provider, model }` file.
#[derive(Debug, Clone, PartialEq)]
pub enum Legacy {
    Missing,
    Model {
        path: PathBuf,
        provider: String,
        model: String,
    },
    Invalid {
        path: PathBuf,
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub config: Config,
    pub legacy: Legacy,
    /// `(path, error)` of an unreadable global or project file.
    pub invalid: Option<(PathBuf, String)>,
    pub project_ignored: bool,
}

pub fn global_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(format!("{NAME}.json"))
}

pub fn project_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join(format!("{NAME}.json"))
}

pub fn legacy_path(agent_dir: &Path) -> PathBuf {
    match std::env::var_os(LEGACY_ENV) {
        Some(path) if !path.is_empty() => PathBuf::from(path),
        _ => agent_dir.join(format!("{NAME}.json")),
    }
}

pub fn state_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(NAME)
}

/// `Ok(None)` when the file is missing.
pub fn read_object(path: &Path) -> Result<Option<Value>, String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.to_string()),
    };
    match serde_json::from_str::<Value>(&raw) {
        Ok(value) if value.is_object() => Ok(Some(value)),
        Ok(_) => Err("Expected a JSON object".to_owned()),
        Err(err) => Err(format!("Invalid JSON: {err}")),
    }
}

pub fn read_legacy(path: PathBuf) -> Legacy {
    let value = match read_object(&path) {
        Ok(None) => return Legacy::Missing,
        Ok(Some(value)) => value,
        Err(error) => {
            let error = if error == "Expected a JSON object" {
                "Expected a JSON object with provider and model fields".to_owned()
            } else {
                error
            };
            return Legacy::Invalid { path, error };
        }
    };
    let text = |key: &str| {
        value[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let Some(provider) = text("provider") else {
        let error = "Missing required string field: provider".to_owned();
        return Legacy::Invalid { path, error };
    };
    match text("model").or_else(|| text("modelId")) {
        Some(model) => Legacy::Model {
            path,
            provider,
            model,
        },
        None => Legacy::Invalid {
            path,
            error: "Missing required string field: model".to_owned(),
        },
    }
}

pub fn load(agent_dir: &Path, cwd: &Path, trusted: bool) -> Loaded {
    let legacy = read_legacy(legacy_path(agent_dir));
    let mut merged = json!({});
    let mut invalid = None;
    let project = project_path(cwd);
    let project_ignored = !trusted && project.exists();
    let mut paths = vec![global_path(agent_dir)];
    if trusted {
        paths.push(project);
    }
    for path in paths {
        match read_object(&path) {
            Ok(Some(layer)) => merge_patch(&mut merged, &layer),
            Ok(None) => {}
            Err(error) => invalid = invalid.or(Some((path, error))),
        }
    }
    let config = match serde_json::from_value(merged) {
        Ok(config) => config,
        Err(err) => {
            invalid = invalid.or(Some((global_path(agent_dir), err.to_string())));
            Config::default()
        }
    };
    Loaded {
        config,
        legacy,
        invalid,
        project_ignored,
    }
}

impl Config {
    /// The provider for `tool`: `tools.<tool>`, else `defaultProvider`, else `auto`.
    pub fn choice(&self, tool: &str) -> Result<Choice, String> {
        let id = self.tools.get(tool).or(self.default_provider.as_ref());
        match id {
            None => Ok(Choice::Auto),
            Some(id) => {
                Choice::parse(id).ok_or_else(|| format!("Unknown web search provider \"{id}\""))
            }
        }
    }

    pub fn base_url(&self, api: Api) -> String {
        let configured = self
            .providers
            .get(api.id())
            .and_then(|p| p.base_url.clone());
        let base = configured.filter(|url| !url.trim().is_empty());
        base.unwrap_or_else(|| api.default_base_url().to_owned())
            .trim_end_matches('/')
            .to_owned()
    }

    pub fn max_results(&self, api: Api) -> u32 {
        let configured = self.providers.get(api.id()).and_then(|p| p.max_results);
        configured.unwrap_or(10).clamp(1, 20)
    }
}

/// Applies `patch` to the JSON object at `path` and writes it back.
pub fn update_file(path: &Path, patch: &Value) -> std::io::Result<()> {
    let mut value = read_object(path)
        .ok()
        .flatten()
        .unwrap_or_else(|| json!({}));
    merge_patch(&mut value, patch);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(&value).map_err(std::io::Error::other)?;
    std::fs::write(path, format!("{text}\n"))
}
