//! Claude Code managed policy: reads each switched-on source and selects the docs that Claude
//! Code applies (`managedSourcesBehavior` `first-wins` or `merge`).

use std::path::PathBuf;

use serde_json::Value;

use super::options::Options;
use super::policy_sources::{self as sources, Hive, PolicyRead};
use crate::paths::Paths;

pub const FILES_SWITCH: &str = "managed/claudeFiles";
pub const MDM_SWITCH: &str = "managed/claudeMdm";
pub const REGISTRY_SWITCH: &str = "managed/claudeRegistry";
pub const SERVER_SWITCH: &str = "managed/claudeServer";

/// Keys that do not make a source deliver policy, besides the `$` metadata keys.
const CONTROL_KEYS: [&str; 2] = ["managedSourcesBehavior", "wslInheritsWindowsSettings"];

/// Claude Code's managed sources, highest rank first. `Mdm` is the plist or the HKLM value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    Remote,
    Mdm,
    Files,
    Hkcu,
}

impl PolicySource {
    /// The server-managed copy and HKCU, which the user can write. Their hooks need trust.
    pub fn user_writable(self) -> bool {
        matches!(self, Self::Remote | Self::Hkcu)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolicyDoc {
    pub source: PolicySource,
    pub path: PathBuf,
    pub doc: Value,
}

struct Found {
    source: PolicySource,
    docs: Vec<(PathBuf, Value)>,
    /// A document exists that cannot be read or parsed.
    broken: bool,
}

impl Found {
    fn new(source: PolicySource, reads: Vec<(PathBuf, PolicyRead)>) -> Self {
        let mut found = Self {
            source,
            docs: Vec::new(),
            broken: false,
        };
        for (path, read) in reads {
            match read {
                PolicyRead::Absent => {}
                PolicyRead::Doc(doc) => found.docs.push((path, doc)),
                PolicyRead::Broken(reason) => {
                    pi_ext_sdk::warn!(
                        "hooks: cannot read Claude managed settings {}: {reason}",
                        path.display()
                    );
                    found.broken = true;
                }
            }
        }
        found
    }

    fn delivers_policy(&self) -> bool {
        self.docs.iter().any(|(_, doc)| {
            doc.as_object().is_some_and(|map| {
                map.iter().any(|(key, value)| {
                    !CONTROL_KEYS.contains(&key.as_str())
                        && !key.starts_with('$')
                        && !value.is_null()
                })
            })
        })
    }

    fn present(&self) -> bool {
        self.broken || self.delivers_policy()
    }

    fn behavior(&self) -> Option<&str> {
        self.docs
            .iter()
            .rev()
            .find_map(|(_, doc)| doc.get("managedSourcesBehavior")?.as_str())
    }
}

/// The managed docs that apply, lowest rank first and in file merge order within a source, so a
/// later doc wins for single values. A source whose switch is off is not read.
pub fn claude_policy_docs(paths: &Paths, options: &Options) -> Vec<PolicyDoc> {
    let on = |key: &str| options.switch_on(key);
    let mut found = Vec::new();
    if on(SERVER_SWITCH) {
        let cache = sources::remote_cache(&paths.claude_dir);
        found.push(Found::new(PolicySource::Remote, vec![cache]));
    }
    let os = paths.claude_os_policy.as_ref();
    let registry = os
        .and_then(|os| os.read_registry)
        .filter(|_| on(REGISTRY_SWITCH));
    let mut mdm = Vec::new();
    if let Some(os) = os.filter(|_| on(MDM_SWITCH)) {
        for path in os.plists.iter().filter(|path| path.exists()) {
            let read = (os.read_plist)(path);
            let readable = matches!(read, PolicyRead::Doc(_));
            mdm.push((path.clone(), read));
            if readable {
                break;
            }
        }
    }
    if mdm.is_empty()
        && let Some(read) = registry
    {
        mdm.push((Hive::Hklm.label(), read(Hive::Hklm)));
    }
    found.push(Found::new(PolicySource::Mdm, mdm));
    if on(FILES_SWITCH)
        && let Some(dir) = &paths.claude_policy
    {
        found.push(Found::new(PolicySource::Files, sources::files(dir)));
    }
    if let Some(read) = registry {
        let hkcu = (Hive::Hkcu.label(), read(Hive::Hkcu));
        found.push(Found::new(PolicySource::Hkcu, vec![hkcu]));
    }
    select(found)
}

/// `found` is in rank order. The user-writable HKCU value applies only when no admin source
/// (remote, MDM, files) is present, and it never merges.
fn select(found: Vec<Found>) -> Vec<PolicyDoc> {
    let (admin, user): (Vec<Found>, Vec<Found>) = found
        .into_iter()
        .partition(|found| found.source != PolicySource::Hkcu);
    let merge = admin
        .iter()
        .find(|found| found.delivers_policy() || found.behavior().is_some())
        .and_then(Found::behavior)
        == Some("merge");
    let admin_present = admin.iter().any(Found::present);
    let mut chosen: Vec<Found> = admin.into_iter().filter(Found::delivers_policy).collect();
    if !merge {
        chosen.truncate(1);
    }
    if !admin_present {
        chosen.extend(user.into_iter().filter(Found::delivers_policy));
    }
    chosen
        .into_iter()
        .rev()
        .flat_map(|found| {
            let source = found.source;
            found.docs.into_iter().map(move |(path, doc)| PolicyDoc {
                source,
                path,
                doc: normalize(doc),
            })
        })
        .collect()
}

/// Claude Code's fallbacks for invalid managed hook keys. An invalid `disableAllHooks` is
/// dropped, which readers of Booleans already do.
fn normalize(mut doc: Value) -> Value {
    let Some(map) = doc.as_object_mut() else {
        return doc;
    };
    if let Some(value) = map.get_mut("allowManagedHooksOnly") {
        let fixed = match value {
            Value::Bool(_) | Value::Null => None,
            Value::String(text) if text == "false" => Some(false),
            _ => Some(true),
        };
        if let Some(fixed) = fixed {
            *value = Value::Bool(fixed);
        }
    }
    for key in ["allowedHttpHookUrls", "httpHookAllowedEnvVars"] {
        if let Some(value) = map.get_mut(key)
            && !value.is_array()
            && !value.is_null()
        {
            *value = Value::Array(Vec::new());
        }
    }
    doc
}

/// A lock such as `allowManagedHooksOnly` or `disableAllHooks`: `true` when the value in force in
/// any source is `true`, else the value in force in the highest source.
pub fn lock(docs: &[PolicyDoc], key: &str) -> Option<bool> {
    let values: Vec<Option<bool>> = docs
        .chunk_by(|a, b| a.source == b.source)
        .map(|group| {
            group
                .iter()
                .rev()
                .find_map(|doc| doc.doc.get(key)?.as_bool())
        })
        .collect();
    if values.contains(&Some(true)) {
        Some(true)
    } else {
        values.last().copied().flatten()
    }
}

#[cfg(test)]
#[path = "claude_policy_tests.rs"]
mod tests;
