//! Readers for each Claude Code managed policy source: the server-managed cache, the macOS
//! managed preferences plist, the Windows registry, and the managed settings files.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};

use super::managed::claude_policy_files;
use super::policy_command::{self, CLAUDE_LIMITS};

/// Claude Code plans for its cached server policy to expire this long after the last fetch.
const REMOTE_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);
const PLIST_DOMAIN: &str = "com.anthropic.claudecode";

#[derive(Debug, PartialEq)]
pub enum PolicyRead {
    Absent,
    Doc(Value),
    /// The document exists but cannot be read or is not a JSON object.
    Broken(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hive {
    Hklm,
    Hkcu,
}

impl Hive {
    pub fn key(self) -> &'static str {
        match self {
            Self::Hklm => r"HKLM\SOFTWARE\Policies\ClaudeCode",
            Self::Hkcu => r"HKCU\SOFTWARE\Policies\ClaudeCode",
        }
    }

    /// The name that managed hooks from this hive show as their file.
    pub fn label(self) -> PathBuf {
        PathBuf::from(format!(r"{}\Settings", self.key()))
    }
}

/// MDM and registry readers. `Paths::from_env` sets the real ones; tests inject their own.
#[derive(Debug, Clone)]
pub struct OsPolicy {
    /// macOS managed preference plists, per-user first. Reading stops at the first readable one.
    pub plists: Vec<PathBuf>,
    pub read_plist: fn(&Path) -> PolicyRead,
    /// `None` where there is no registry.
    pub read_registry: Option<fn(Hive) -> PolicyRead>,
}

impl OsPolicy {
    pub fn from_env() -> Self {
        let plists = if cfg!(target_os = "macos") {
            plist_paths(account_name().as_deref())
        } else {
            Vec::new()
        };
        let registry: fn(Hive) -> PolicyRead = reg_query;
        Self {
            plists,
            read_plist: plutil,
            read_registry: cfg!(windows).then_some(registry),
        }
    }
}

/// The per-user plist of `user`, then the device plist.
fn plist_paths(user: Option<&str>) -> Vec<PathBuf> {
    let dir = Path::new("/Library/Managed Preferences");
    let file = format!("{PLIST_DOMAIN}.plist");
    let mut plists: Vec<PathBuf> = user.iter().map(|user| dir.join(user).join(&file)).collect();
    plists.push(dir.join(file));
    plists
}

/// The process account name, as Claude Code reads it with `os.userInfo()`.
fn account_name() -> Option<String> {
    let output = Command::new("/usr/bin/id").arg("-un").output().ok()?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !name.is_empty()).then_some(name)
}

/// A JSON object document. Empty text counts as `{}`, as Claude Code reads an empty file.
pub fn parse_doc(text: &str) -> PolicyRead {
    if text.trim().is_empty() {
        return PolicyRead::Doc(Value::Object(Map::new()));
    }
    match serde_json::from_str::<Value>(text) {
        Ok(doc @ Value::Object(_)) => PolicyRead::Doc(doc),
        Ok(_) => PolicyRead::Broken("the top level is not a JSON object".to_owned()),
        Err(err) => PolicyRead::Broken(err.to_string()),
    }
}

pub fn read_file(path: &Path) -> PolicyRead {
    match std::fs::read_to_string(path) {
        Ok(text) => parse_doc(&text),
        Err(err) if err.kind() == ErrorKind::NotFound => PolicyRead::Absent,
        Err(err) => PolicyRead::Broken(err.to_string()),
    }
}

/// `managed-settings.json` and `managed-settings.d/*.json`, in merge order.
pub fn files(dir: &Path) -> Vec<(PathBuf, PolicyRead)> {
    let mut reads: Vec<(PathBuf, PolicyRead)> = claude_policy_files(dir)
        .into_iter()
        .map(|path| {
            let read = read_file(&path);
            (path, read)
        })
        .collect();
    let drop_ins = dir.join("managed-settings.d");
    if let Err(err) = std::fs::read_dir(&drop_ins)
        && err.kind() != ErrorKind::NotFound
    {
        reads.push((drop_ins, PolicyRead::Broken(err.to_string())));
    }
    reads
}

/// The `remote-settings.json` copy of the server-managed settings, unless it has expired. Claude
/// Code reads a copy that cannot be read or parsed as absent.
pub fn remote_cache(claude_dir: &Path) -> (PathBuf, PolicyRead) {
    let path = claude_dir.join("remote-settings.json");
    // A signed fetch that finds the settings unchanged rewrites only the signature file.
    let signature = claude_dir.join("remote-settings.json.signature.json");
    let age = [&path, &signature]
        .into_iter()
        .filter_map(|file| {
            std::fs::metadata(file)
                .and_then(|meta| meta.modified())
                .ok()
        })
        .max()
        .and_then(|fetched| SystemTime::now().duration_since(fetched).ok());
    if age.is_some_and(|age| age > REMOTE_CACHE_TTL) {
        pi_ext_sdk::info!(
            "hooks: {} is older than 24 hours; skipping it",
            path.display()
        );
        return (path, PolicyRead::Absent);
    }
    match read_file(&path) {
        PolicyRead::Broken(reason) => {
            pi_ext_sdk::warn!("hooks: skipping {}: {reason}", path.display());
            (path, PolicyRead::Absent)
        }
        read => (path, read),
    }
}

fn plutil(path: &Path) -> PolicyRead {
    let mut command = Command::new("/usr/bin/plutil");
    command
        .args(["-convert", "json", "-o", "-", "--"])
        .arg(path);
    match policy_command::run(&mut command, CLAUDE_LIMITS) {
        Ok(out) if out.status.success() => parse_doc(&String::from_utf8_lossy(&out.stdout)),
        Ok(out) => PolicyRead::Broken(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
        Err(err) => PolicyRead::Broken(format!("plutil: {err}")),
    }
}

/// `reg query <hive key> /v Settings`. A missing key or value makes `reg` exit non-zero.
fn reg_query(hive: Hive) -> PolicyRead {
    let root = std::env::var_os("SystemRoot")
        .filter(|root| !root.is_empty())
        .map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    let mut command = Command::new(root.join("System32").join("reg.exe"));
    command.args(["query", hive.key(), "/v", "Settings"]);
    match policy_command::run(&mut command, CLAUDE_LIMITS) {
        Ok(out) if out.status.success() => parse_reg_query(&String::from_utf8_lossy(&out.stdout)),
        Ok(_) => PolicyRead::Absent,
        Err(err) => PolicyRead::Broken(format!("reg: {err}")),
    }
}

/// The `Settings` value from `reg query` output: a `REG_SZ` or `REG_EXPAND_SZ` JSON object.
pub fn parse_reg_query(output: &str) -> PolicyRead {
    let Some(rest) = output.lines().find_map(|line| {
        let rest = line.trim_start().strip_prefix("Settings")?;
        rest.starts_with(char::is_whitespace)
            .then_some(rest.trim_start())
    }) else {
        return PolicyRead::Absent;
    };
    let (kind, data) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
    if !matches!(kind, "REG_SZ" | "REG_EXPAND_SZ") {
        return PolicyRead::Broken(format!("Settings is {kind}, not a string"));
    }
    let data = data.trim();
    if data.is_empty() {
        return PolicyRead::Broken("Settings is empty".to_owned());
    }
    parse_doc(data)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn reg_query_output_gives_the_settings_json() {
        let output = "\r\nHKEY_LOCAL_MACHINE\\SOFTWARE\\Policies\\ClaudeCode\r\n    Settings    REG_SZ    {\"disableAllHooks\": true}\r\n\r\n";
        assert_eq!(
            parse_reg_query(output),
            PolicyRead::Doc(json!({ "disableAllHooks": true }))
        );
        let expand = "    Settings    REG_EXPAND_SZ    {}\n";
        assert_eq!(parse_reg_query(expand), PolicyRead::Doc(json!({})));
        assert_eq!(
            parse_reg_query("    SettingsX    REG_SZ    {}"),
            PolicyRead::Absent
        );
        let dword = "    Settings    REG_DWORD    0x1";
        assert!(matches!(parse_reg_query(dword), PolicyRead::Broken(_)));
        assert!(matches!(
            parse_reg_query("    Settings    REG_SZ    "),
            PolicyRead::Broken(_)
        ));
        assert!(matches!(
            parse_reg_query("    Settings    REG_SZ    [1]"),
            PolicyRead::Broken(_)
        ));
    }

    #[test]
    fn empty_files_are_empty_objects_and_bad_json_is_broken() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("managed-settings.json"), "").unwrap();
        let reads = files(dir.path());
        assert_eq!(reads[0].1, PolicyRead::Doc(json!({})));
        std::fs::write(dir.path().join("managed-settings.json"), "{").unwrap();
        assert!(matches!(files(dir.path())[0].1, PolicyRead::Broken(_)));
    }

    fn set_modified(path: &Path, time: SystemTime) {
        let file = std::fs::File::options().write(true).open(path).unwrap();
        file.set_modified(time).unwrap();
    }

    #[test]
    fn an_unparsable_or_expired_remote_cache_is_absent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remote-settings.json");
        std::fs::write(&path, "{").unwrap();
        assert_eq!(remote_cache(dir.path()).1, PolicyRead::Absent);
        std::fs::write(&path, r#"{"disableAllHooks": true}"#).unwrap();
        assert!(matches!(remote_cache(dir.path()).1, PolicyRead::Doc(_)));
        let old = SystemTime::now() - REMOTE_CACHE_TTL - Duration::from_secs(60);
        set_modified(&path, old);
        assert_eq!(remote_cache(dir.path()).1, PolicyRead::Absent);
        let signature = dir.path().join("remote-settings.json.signature.json");
        std::fs::write(&signature, r#"{"jws": "a.b.c", "receivedAt": 0}"#).unwrap();
        assert!(matches!(remote_cache(dir.path()).1, PolicyRead::Doc(_)));
        set_modified(&signature, old);
        assert_eq!(remote_cache(dir.path()).1, PolicyRead::Absent);
    }

    #[test]
    fn the_per_user_plist_comes_first() {
        let dir = Path::new("/Library/Managed Preferences");
        let device = dir.join("com.anthropic.claudecode.plist");
        let user = dir.join("ada/com.anthropic.claudecode.plist");
        assert_eq!(plist_paths(Some("ada")), [user, device.clone()]);
        assert_eq!(plist_paths(None), [device]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn plutil_converts_a_plist_to_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.plist");
        let plist = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>allowManagedHooksOnly</key><true/>
<key>allowedHttpHookUrls</key><array><string>https://hooks.example.com/*</string></array>
</dict></plist>"#;
        std::fs::write(&path, plist).unwrap();
        let expected = json!({
            "allowManagedHooksOnly": true,
            "allowedHttpHookUrls": ["https://hooks.example.com/*"],
        });
        assert_eq!(plutil(&path), PolicyRead::Doc(expected));
        std::fs::write(&path, "not a plist").unwrap();
        assert!(matches!(plutil(&path), PolicyRead::Broken(_)));
    }
}
