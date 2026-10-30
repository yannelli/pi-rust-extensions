use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use pi_ext_sdk::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Profile {
    pub host: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub identity_file: Option<String>,
    #[serde(default)]
    pub jump_host: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub options: BTreeMap<String, String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn default_port() -> u16 {
    22
}
fn enabled() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
    #[serde(default = "default_persist")]
    pub control_persist: u64,
}

fn default_persist() -> u64 {
    600
}

impl Default for Config {
    fn default() -> Self {
        Self {
            profiles: BTreeMap::new(),
            control_persist: default_persist(),
        }
    }
}

pub fn paths(host: &Host) -> (PathBuf, PathBuf) {
    (
        host.agent_dir().join("ext/ssh.json"),
        host.cwd().join(".pi/ext/ssh.json"),
    )
}

pub fn read(path: &Path) -> Result<Value> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(json!({})),
        Err(error) => Err(error.into()),
    }
}

pub fn merge(mut global: Value, project: Value) -> Result<Config> {
    pi_ext_sdk::json::merge_patch(&mut global, &project);
    let config: Config = serde_json::from_value(global)?;
    for (name, profile) in &config.profiles {
        validate(name, profile)?;
    }
    if config.control_persist == 0 {
        return Err(Error::msg("controlPersist must be greater than zero"));
    }
    Ok(config)
}

pub async fn trusted(host: &Host) -> Result<bool> {
    Ok(host
        .pi_call("ctx", "isProjectTrusted", vec![])
        .await?
        .as_bool()
        == Some(true))
}

pub async fn load(host: &Host, warned: &AtomicBool) -> Result<Config> {
    let (global, project) = paths(host);
    let project_config = if project.exists() {
        if trusted(host).await? {
            read(&project)?
        } else {
            if !warned.swap(true, Ordering::Relaxed) {
                host.notify_warning("SSH project config ignored: project is not trusted")
                    .await?;
            }
            json!({})
        }
    } else {
        json!({})
    };
    merge(read(&global)?, project_config)
}

pub fn validate(name: &str, profile: &Profile) -> Result<()> {
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(Error::msg(
            "Profile name must be nonempty and contain no control characters",
        ));
    }
    if profile.host.is_empty()
        || profile.host.starts_with('-')
        || profile
            .host
            .chars()
            .any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(Error::msg("Invalid SSH host"));
    }
    if profile.port == 0 {
        return Err(Error::msg("Invalid SSH port"));
    }
    for key in profile.env.keys() {
        if !valid_env_key(key) {
            return Err(Error::msg(format!("Invalid environment name: {key}")));
        }
    }
    for (key, value) in &profile.options {
        if !key.chars().all(|c| c.is_ascii_alphabetic())
            || key.is_empty()
            || value.contains(['\n', '\r', '\0'])
        {
            return Err(Error::msg("Invalid SSH option"));
        }
        if [
            "controlpath",
            "controlmaster",
            "controlpersist",
            "batchmode",
            "requesttty",
            "remotecommand",
            "sessiontype",
            "forkafterauthentication",
            "stdinnull",
        ]
        .contains(&key.to_lowercase().as_str())
        {
            return Err(Error::msg(format!(
                "SSH option is managed by the extension: {key}"
            )));
        }
    }
    Ok(())
}

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    chars
        .next()
        .is_some_and(|c| c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

pub fn save(path: &Path, value: &Value) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::msg("Config path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    use std::io::Write;
    file.write_all(&serde_json::to_vec_pretty(value)?)?;
    file.sync_all()?;
    std::fs::rename(temp, path)?;
    Ok(())
}
