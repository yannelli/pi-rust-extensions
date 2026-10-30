use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub const DEFAULT_PORT: u16 = 9222;

pub const KEYS: &[(&str, &str)] = &[
    ("mode", "auto | connect | launch"),
    ("host", "DevTools host, default 127.0.0.1"),
    ("port", "DevTools port, default 9222"),
    ("chromePath", "Chrome binary for the managed profile"),
    ("userDataDir", "Managed profile folder"),
    ("headless", "true | false, managed profile only"),
    ("background", "true | false, keep tabs in the background"),
    (
        "defaultAuthorizeMinutes",
        "Minutes for /chrome authorize, 0 = until revoked",
    ),
    (
        "closeOnExit",
        "true | false, close the managed Chrome on quit",
    ),
];

/// One config file. Every key is optional so project files override per key.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ConfigFile {
    pub mode: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub chrome_path: Option<String>,
    pub user_data_dir: Option<String>,
    pub headless: Option<bool>,
    pub extra_args: Option<Vec<String>>,
    pub background: Option<bool>,
    pub default_authorize_minutes: Option<u64>,
    pub close_on_exit: Option<bool>,
}

impl ConfigFile {
    pub fn merge(global: Self, project: Self) -> Self {
        Self {
            mode: project.mode.or(global.mode),
            host: project.host.or(global.host),
            port: project.port.or(global.port),
            chrome_path: project.chrome_path.or(global.chrome_path),
            user_data_dir: project.user_data_dir.or(global.user_data_dir),
            headless: project.headless.or(global.headless),
            extra_args: project.extra_args.or(global.extra_args),
            background: project.background.or(global.background),
            default_authorize_minutes: project
                .default_authorize_minutes
                .or(global.default_authorize_minutes),
            close_on_exit: project.close_on_exit.or(global.close_on_exit),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Connect,
    Launch,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub mode: Mode,
    pub host: String,
    pub port: u16,
    pub chrome_path: Option<String>,
    pub user_data_dir: PathBuf,
    pub headless: bool,
    pub extra_args: Vec<String>,
    pub background: bool,
    pub default_authorize_minutes: u64,
    pub close_on_exit: bool,
}

impl Config {
    pub fn resolve(file: ConfigFile, state_dir: &Path) -> Self {
        let mode = match file.mode.as_deref() {
            Some("connect") => Mode::Connect,
            Some("launch") => Mode::Launch,
            _ => Mode::Auto,
        };
        Self {
            mode,
            host: file.host.unwrap_or_else(|| "127.0.0.1".to_owned()),
            port: file.port.unwrap_or(DEFAULT_PORT),
            chrome_path: file.chrome_path,
            user_data_dir: file
                .user_data_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| state_dir.join("profile")),
            headless: file.headless.unwrap_or(false),
            extra_args: file.extra_args.unwrap_or_default(),
            background: file.background.unwrap_or(true),
            default_authorize_minutes: file.default_authorize_minutes.unwrap_or(15),
            close_on_exit: file.close_on_exit.unwrap_or(true),
        }
    }

    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            Mode::Auto => "auto",
            Mode::Connect => "connect",
            Mode::Launch => "launch",
        }
    }
}

pub fn global_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("chrome.json")
}

pub fn project_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join("chrome.json")
}

pub fn state_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("chrome")
}

/// Reads a config file. A missing file is empty; a broken file is an error.
pub fn read_file(path: &Path) -> Result<ConfigFile> {
    match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("{}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(ConfigFile::default()),
        Err(err) => Err(err).with_context(|| format!("{}", path.display())),
    }
}

/// Parses user input for one key into JSON. Empty input removes the key.
pub fn parse_value(key: &str, input: &str) -> Result<Option<Value>> {
    let input = input.trim();
    if input.is_empty() {
        return Ok(None);
    }
    let value = match key {
        "port" | "defaultAuthorizeMinutes" => Value::from(
            input
                .parse::<u64>()
                .with_context(|| format!("{key} must be a number"))?,
        ),
        "headless" | "background" | "closeOnExit" => match input {
            "true" | "on" | "yes" => Value::Bool(true),
            "false" | "off" | "no" => Value::Bool(false),
            _ => anyhow::bail!("{key} must be true or false"),
        },
        "mode" if !matches!(input, "auto" | "connect" | "launch") => {
            anyhow::bail!("mode must be auto, connect, or launch")
        }
        _ => Value::String(input.to_owned()),
    };
    Ok(Some(value))
}

pub fn write_key(path: &Path, key: &str, value: Option<Value>) -> Result<()> {
    let mut object: Map<String, Value> = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).with_context(|| format!("{}", path.display()))?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(err) => return Err(err).with_context(|| format!("{}", path.display())),
    };
    match value {
        Some(value) => object.insert(key.to_owned(), value),
        None => object.remove(key),
    };
    serde_json::from_value::<ConfigFile>(Value::Object(object.clone()))
        .with_context(|| format!("invalid value for {key}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(object))?;
    std::fs::write(path, text + "\n").with_context(|| format!("{}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_wins_per_key() {
        let global = ConfigFile {
            port: Some(9333),
            headless: Some(true),
            ..ConfigFile::default()
        };
        let project = ConfigFile {
            port: Some(9444),
            ..ConfigFile::default()
        };
        let merged = ConfigFile::merge(global, project);
        assert_eq!(merged.port, Some(9444));
        assert_eq!(merged.headless, Some(true));
    }

    #[test]
    fn defaults_apply() {
        let config = Config::resolve(ConfigFile::default(), Path::new("/a/ext/chrome"));
        assert_eq!(config.mode, Mode::Auto);
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.user_data_dir, PathBuf::from("/a/ext/chrome/profile"));
        assert!(config.background && config.close_on_exit && !config.headless);
        assert_eq!(config.default_authorize_minutes, 15);
    }

    #[test]
    fn write_key_round_trips_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ext/chrome.json");
        write_key(&path, "port", parse_value("port", "9333").unwrap()).unwrap();
        write_key(&path, "mode", parse_value("mode", "connect").unwrap()).unwrap();
        assert_eq!(read_file(&path).unwrap().port, Some(9333));
        write_key(&path, "port", None).unwrap();
        let file = read_file(&path).unwrap();
        assert_eq!((file.port, file.mode.as_deref()), (None, Some("connect")));
        assert!(parse_value("mode", "remote").is_err());
        assert!(parse_value("headless", "maybe").is_err());
        assert!(write_key(&path, "port", Some(Value::from(99999))).is_err());
    }
}
