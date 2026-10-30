//! The pi-owned files: `<agentDir>/ext/permissions.json` and `<cwd>/.pi/ext/permissions.json`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::rules::{Decision, Source};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    #[default]
    Global,
    Project,
}

impl Scope {
    pub fn name(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }

    pub fn other(self) -> Self {
        match self {
            Scope::Global => Scope::Project,
            Scope::Project => Scope::Global,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PiFile {
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub sources: BTreeMap<String, bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<Scope>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ask: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl PiFile {
    /// A missing file is empty; a file that does not parse is an error.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let write = || -> std::io::Result<()> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let text = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
            std::fs::write(path, text + "\n")
        };
        write().map_err(|e| format!("cannot write {}: {e}", path.display()))
    }

    pub fn list(&self, decision: Decision) -> &Vec<String> {
        match decision {
            Decision::Allow => &self.allow,
            Decision::Ask => &self.ask,
            Decision::Deny => &self.deny,
        }
    }

    pub fn list_mut(&mut self, decision: Decision) -> &mut Vec<String> {
        match decision {
            Decision::Allow => &mut self.allow,
            Decision::Ask => &mut self.ask,
            Decision::Deny => &mut self.deny,
        }
    }

    /// Adds `rule` to `decision` and removes it from the other lists.
    pub fn add(&mut self, decision: Decision, rule: &str) {
        for d in Decision::ALL {
            self.list_mut(d).retain(|r| r != rule);
        }
        self.list_mut(decision).push(rule.to_owned());
    }

    pub fn remove(&mut self, rule: &str) -> bool {
        let before = self.allow.len() + self.ask.len() + self.deny.len();
        for d in Decision::ALL {
            self.list_mut(d).retain(|r| r != rule);
        }
        before != self.allow.len() + self.ask.len() + self.deny.len()
    }
}

/// Loads, changes, and writes one pi file.
pub fn edit(path: &Path, change: impl FnOnce(&mut PiFile)) -> Result<(), String> {
    let mut file = PiFile::load(path)?;
    change(&mut file);
    file.save(path)
}

pub fn global_file(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("permissions.json")
}

pub fn project_file(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join("permissions.json")
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Options {
    sources: BTreeMap<String, bool>,
    pub scope: Scope,
}

impl Options {
    /// The project file wins per key; every source is on unless a file turns it off.
    pub fn merge(global: &PiFile, project: Option<&PiFile>) -> Self {
        let mut sources = global.sources.clone();
        let mut scope = global.scope;
        if let Some(project) = project {
            sources.extend(project.sources.clone());
            scope = project.scope.or(scope);
        }
        Self {
            sources,
            scope: scope.unwrap_or_default(),
        }
    }

    pub fn on(&self, source: Source) -> bool {
        self.key_on(source.key())
    }

    pub fn key_on(&self, key: &str) -> bool {
        self.sources.get(key).copied().unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_round_trip_and_keep_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ext/permissions.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"deny": ["Bash(rm *)"], "note": "kept"}"#).unwrap();
        edit(&path, |f| {
            f.add(Decision::Allow, "Bash(rm *)");
            f.sources.insert("codex-user".into(), false);
        })
        .unwrap();
        let file = PiFile::load(&path).unwrap();
        assert!(file.deny.is_empty());
        assert_eq!(file.allow, ["Bash(rm *)"]);
        assert_eq!(file.extra["note"], "kept");
        let mut again = file.clone();
        assert!(again.remove("Bash(rm *)"));
        assert!(!again.remove("Bash(rm *)"));
        std::fs::write(&path, "{").unwrap();
        assert!(PiFile::load(&path).is_err());
        assert_eq!(
            PiFile::load(&dir.path().join("none.json")).unwrap(),
            PiFile::default()
        );
    }

    #[test]
    fn project_options_win() {
        let global = PiFile {
            sources: BTreeMap::from([("codex-user".into(), false), ("pi".into(), false)]),
            scope: Some(Scope::Project),
            ..PiFile::default()
        };
        let project = PiFile {
            sources: BTreeMap::from([("pi".into(), true)]),
            scope: Some(Scope::Global),
            ..PiFile::default()
        };
        let only_global = Options::merge(&global, None);
        assert_eq!(only_global.scope, Scope::Project);
        assert!(!only_global.on(Source::PiProject));
        let both = Options::merge(&global, Some(&project));
        assert_eq!(both.scope, Scope::Global);
        assert!(both.on(Source::PiGlobal));
        assert!(!both.on(Source::CodexUser));
        assert!(both.on(Source::ClaudeUser));
    }
}
