use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// `<agentDir>/ext/hooks/state.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TrustState {
    pub trusted: BTreeSet<String>,
    pub denied: BTreeSet<String>,
    pub disabled: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    All,
    Trusted,
    Once,
    Denied,
    Untrusted,
}

impl Trust {
    pub fn name(self) -> &'static str {
        match self {
            Trust::All => "trustAll",
            Trust::Trusted => "trusted",
            Trust::Once => "once",
            Trust::Denied => "denied",
            Trust::Untrusted => "untrusted",
        }
    }
}

impl TrustState {
    pub fn load(path: &Path) -> Self {
        load_json(path).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        save_json(path, self);
    }

    pub fn trust(&mut self, hash: &str) {
        self.denied.remove(hash);
        self.trusted.insert(hash.to_owned());
    }

    pub fn deny(&mut self, hash: &str) {
        self.trusted.remove(hash);
        self.denied.insert(hash.to_owned());
    }

    pub fn untrust(&mut self, hash: &str) {
        self.trusted.remove(hash);
        self.denied.remove(hash);
    }

    pub fn level(&self, hash: &str, trust_all: bool, once: bool) -> Trust {
        if self.denied.contains(hash) {
            Trust::Denied
        } else if trust_all {
            Trust::All
        } else if self.trusted.contains(hash) {
            Trust::Trusted
        } else if once {
            Trust::Once
        } else {
            Trust::Untrusted
        }
    }
}

/// One entry of `last-runs.json`, keyed by hook hash.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LastRun {
    pub at: u64,
    pub duration_ms: u64,
    pub exit: Option<i32>,
    pub result: String,
    pub stdout: String,
    pub stderr: String,
}

pub type LastRuns = BTreeMap<String, LastRun>;

pub fn load_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text)
        .map_err(|err| pi_ext_sdk::warn!("hooks: cannot read {}: {err}", path.display()))
        .ok()
}

pub fn save_json<T: Serialize>(path: &Path, value: &T) {
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(value).map_err(std::io::Error::other)?;
        std::fs::write(path, text + "\n")
    };
    if let Err(err) = write() {
        pi_ext_sdk::warn!("hooks: cannot write {}: {err}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_persists() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ext/hooks/state.json");
        let mut state = TrustState::default();
        state.trust("a");
        state.deny("b");
        state.disabled.insert("c".into());
        state.save(&path);
        let loaded = TrustState::load(&path);
        assert_eq!(loaded, state);
        assert_eq!(loaded.level("a", false, false), Trust::Trusted);
        assert_eq!(loaded.level("b", true, true), Trust::Denied);
        assert_eq!(loaded.level("x", true, false), Trust::All);
        assert_eq!(loaded.level("x", false, true), Trust::Once);
        assert_eq!(loaded.level("x", false, false), Trust::Untrusted);
        let mut again = loaded;
        again.deny("a");
        assert!(!again.trusted.contains("a"));
        again.untrust("a");
        assert!(!again.denied.contains("a"));
        assert!(again.disabled.contains("c"));
    }
}
