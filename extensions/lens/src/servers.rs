//! LSP server entries: the fields of one server, and the server files that plugins write.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Server file in each scope: `<agentDir>/ext/lens-lsp.json` and `<cwd>/.pi/ext/lens-lsp.json`.
pub const FILE: &str = "ext/lens-lsp.json";
/// Upper bound for `shutdownTimeout`, so one server cannot hold a scan for long.
pub const MAX_SHUTDOWN_MS: u64 = 5_000;
const DEFAULT_SHUTDOWN_MS: u64 = 500;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LspServer {
    /// The key in a server file. Lens names findings and stalls after it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The owner of the entry, such as `<plugin>@<marketplace>`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    pub command: String,
    pub args: Vec<String>,
    pub languages: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extension_to_language: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initialization_options: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shutdown_timeout: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_timeout: Option<u64>,
    pub diagnostics: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restart_on_crash: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_restarts: Option<u64>,
    pub enabled: bool,
    #[serde(flatten, skip_serializing)]
    pub unknown: BTreeMap<String, Value>,
}

impl Default for LspServer {
    fn default() -> Self {
        Self {
            name: None,
            plugin: None,
            command: String::new(),
            args: Vec::new(),
            languages: Vec::new(),
            extension_to_language: BTreeMap::new(),
            env: BTreeMap::new(),
            initialization_options: None,
            settings: None,
            workspace_folder: None,
            startup_timeout: None,
            shutdown_timeout: None,
            request_timeout: None,
            diagnostics: true,
            transport: None,
            restart_on_crash: None,
            max_restarts: None,
            enabled: true,
            unknown: BTreeMap::new(),
        }
    }
}

fn extension(path: &Path) -> Option<&str> {
    path.extension()?.to_str()
}

impl LspServer {
    /// The name, else the command. Findings and stall records use it.
    pub fn label(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.command)
    }

    /// The error that makes this entry unusable.
    pub fn problem(&self) -> Option<&'static str> {
        if self.command.is_empty() {
            return Some("needs command");
        }
        if self.languages.is_empty() && self.extension_to_language.is_empty() {
            return Some("needs languages or extensionToLanguage");
        }
        let zero = [
            self.startup_timeout,
            self.shutdown_timeout,
            self.request_timeout,
        ];
        if zero.contains(&Some(0)) {
            return Some("timeouts must be positive");
        }
        None
    }

    /// Fields that Lens reads but does not apply, and unknown fields.
    pub fn warnings(&self) -> Vec<String> {
        let label = self.label();
        let mut out: Vec<String> = self
            .unknown
            .keys()
            .map(|key| format!("Lens LSP server {label}: unknown field \"{key}\" ignored"))
            .collect();
        if let Some(transport) = self.transport.as_deref()
            && !matches!(transport, "stdio" | "socket")
        {
            out.push(format!(
                "Lens LSP server {label}: transport \"{transport}\" is not supported; Lens uses stdio"
            ));
        }
        if self.restart_on_crash.is_some() || self.max_restarts.is_some() {
            out.push(format!("Lens LSP server {label}: restartOnCrash and maxRestarts are not applied, because Lens starts a new server for each request"));
        }
        out
    }

    /// The language ID that `extensionToLanguage` maps the file extension of `path` to.
    pub fn mapped_language(&self, path: &Path) -> Option<&str> {
        let ext = extension(path)?;
        self.extension_to_language
            .iter()
            .find(|(key, _)| key.trim_start_matches('.') == ext)
            .map(|(_, language)| language.as_str())
    }

    /// True when this server checks `path`, whose Lens language is `language`.
    pub fn handles(&self, path: &Path, language: &str) -> bool {
        self.mapped_language(path).is_some() || self.languages.iter().any(|item| item == language)
    }

    /// `workspaceFolder` (relative to `cwd`), else the detected project `root`.
    pub fn root(&self, cwd: &Path, root: &Path) -> PathBuf {
        match self
            .workspace_folder
            .as_deref()
            .filter(|dir| !dir.is_empty())
        {
            Some(dir) => cwd.join(dir),
            None => root.to_path_buf(),
        }
    }

    /// The wait after `shutdown` and `exit`: `shutdownTimeout`, at most [`MAX_SHUTDOWN_MS`].
    pub fn shutdown_grace(&self) -> Duration {
        let ms = self.shutdown_timeout.unwrap_or(DEFAULT_SHUTDOWN_MS);
        Duration::from_millis(ms.min(MAX_SHUTDOWN_MS))
    }
}

/// The server files of the user and project scopes.
pub fn paths(agent_dir: &Path, cwd: &Path) -> [PathBuf; 2] {
    [
        agent_dir.join(FILE),
        pi_ext_sdk::paths::project_path(cwd, Path::new(".pi").join(FILE)),
    ]
}

/// Modification times of the server files, to notice changes between scans.
pub fn stamp(agent_dir: &Path, cwd: &Path) -> Vec<Option<SystemTime>> {
    paths(agent_dir, cwd)
        .iter()
        .map(|path| std::fs::metadata(path).and_then(|m| m.modified()).ok())
        .collect()
}

fn read_servers(path: &Path, warnings: &mut Vec<String>) -> serde_json::Map<String, Value> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Default::default(),
        Err(error) => {
            warnings.push(format!("Lens: cannot read {}: {error}", path.display()));
            return Default::default();
        }
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!(
                "Lens: {} is not valid JSON: {error}",
                path.display()
            ));
            return Default::default();
        }
    };
    let Value::Object(mut file) = value else {
        warnings.push(format!("Lens: {} is not an object", path.display()));
        return Default::default();
    };
    let servers = match file.remove("servers") {
        Some(Value::Object(servers)) => servers,
        None => Default::default(),
        Some(_) => {
            warnings.push(format!(
                "Lens: servers in {} is not an object",
                path.display()
            ));
            Default::default()
        }
    };
    for key in file.keys() {
        warnings.push(format!(
            "Lens: unknown field \"{key}\" in {} ignored",
            path.display()
        ));
    }
    servers
}

/// Servers of the server files: user, then project when `trusted`. A project entry
/// replaces a user entry of the same name. Returns the servers and whether trust hid any.
pub fn load(
    agent_dir: &Path,
    cwd: &Path,
    trusted: bool,
    warnings: &mut Vec<String>,
) -> (Vec<LspServer>, bool) {
    let [user, project] = paths(agent_dir, cwd);
    let mut merged = read_servers(&user, warnings);
    let mut ignored = false;
    if trusted {
        merged.extend(read_servers(&project, warnings));
    } else {
        ignored = project.is_file();
    }
    let mut out = Vec::new();
    for (name, value) in merged {
        let mut server = match serde_json::from_value::<LspServer>(value) {
            Ok(server) => server,
            Err(error) => {
                warnings.push(format!("Lens LSP server {name} skipped: {error}"));
                continue;
            }
        };
        server.name = Some(name.clone());
        if let Some(problem) = server.problem() {
            warnings.push(format!("Lens LSP server {name} skipped: it {problem}"));
            continue;
        }
        out.push(server);
    }
    (out, ignored)
}
