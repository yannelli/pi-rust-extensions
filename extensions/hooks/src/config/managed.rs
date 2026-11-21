//! Organization policy files: Claude Code managed settings and Codex `requirements.toml`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::codex::read_toml;
use super::options::read_json;

/// `managed-settings.json`, then `managed-settings.d/*.json` in name order without hidden files.
pub fn claude_policy_files(dir: &Path) -> Vec<PathBuf> {
    let mut drop_ins: Vec<PathBuf> = std::fs::read_dir(dir.join("managed-settings.d"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter(|path| {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or(".");
            !name.starts_with('.')
        })
        .collect();
    drop_ins.sort();
    let mut files = vec![dir.join("managed-settings.json")];
    files.extend(drop_ins);
    files
}

pub fn claude_policy_docs(dir: &Path) -> Vec<(PathBuf, Value)> {
    claude_policy_files(dir)
        .into_iter()
        .filter_map(|path| read_json(&path).map(|doc| (path, doc)))
        .collect()
}

/// `enabledPlugins` by `plugin@marketplace` id: `true` force-enables, `false` blocks. A later doc
/// wins per id.
pub fn enabled_plugins<'a>(docs: impl Iterator<Item = &'a Value>) -> BTreeMap<String, bool> {
    let mut enabled = BTreeMap::new();
    for plugins in docs.filter_map(|doc| doc.get("enabledPlugins")?.as_object()) {
        for (id, value) in plugins {
            if let Some(on) = value.as_bool() {
                enabled.insert(id.clone(), on);
            }
        }
    }
    enabled
}

/// Codex `requirements.toml` in the system dir.
#[derive(Debug, Default)]
pub struct Requirements {
    pub file: PathBuf,
    /// `[hooks]`, the same shape as `config.toml` inline hooks.
    pub hooks: Option<Value>,
    /// `[features] hooks`, which wins over every config layer.
    pub feature: Option<bool>,
    /// `allow_managed_hooks_only`: only system and `requirements.toml` hooks run.
    pub managed_only: bool,
}

pub fn codex_requirements(system: &Path) -> Requirements {
    let file = system.join("requirements.toml");
    let doc = read_toml(&file).unwrap_or(Value::Null);
    let features = doc.get("features");
    Requirements {
        hooks: doc.get("hooks").filter(|h| h.is_object()).cloned(),
        feature: features
            .and_then(|f| f.get("hooks").or_else(|| f.get("codex_hooks")))
            .and_then(Value::as_bool),
        managed_only: doc.get("allow_managed_hooks_only") == Some(&Value::Bool(true)),
        file,
    }
}

/// Codex `project_root_markers` from the user `config.toml`; `.git` when unset.
pub fn codex_root_markers(codex_home: &Path) -> Vec<String> {
    let doc = read_toml(&codex_home.join("config.toml"));
    match doc.as_ref().and_then(|d| d.get("project_root_markers")) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => vec![".git".to_owned()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drop_ins_follow_the_base_file_in_name_order() {
        let dir = tempfile::tempdir().unwrap();
        let drop_ins = dir.path().join("managed-settings.d");
        std::fs::create_dir_all(&drop_ins).unwrap();
        for name in ["20-b.json", ".hidden.json", "10-a.json", "notes.txt"] {
            std::fs::write(drop_ins.join(name), "{}").unwrap();
        }
        let names: Vec<String> = claude_policy_files(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["managed-settings.json", "10-a.json", "20-b.json"]);
    }

    #[test]
    fn requirements_hooks_and_pins() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("requirements.toml"),
            r#"
allow_managed_hooks_only = true
[features]
hooks = true
[hooks]
managed_dir = "/enterprise/hooks"
[[hooks.PreToolUse]]
matcher = "^Bash$"
[[hooks.PreToolUse.hooks]]
type = "command"
command = "/enterprise/hooks/check.py"
"#,
        )
        .unwrap();
        let req = codex_requirements(dir.path());
        assert!(req.managed_only);
        assert_eq!(req.feature, Some(true));
        let hooks = req.hooks.unwrap();
        assert_eq!(hooks["PreToolUse"][0]["matcher"], "^Bash$");
    }
}
