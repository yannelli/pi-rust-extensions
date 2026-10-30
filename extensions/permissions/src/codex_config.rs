use std::path::Path;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct CodexConfig {
    pub approval_policy: Option<String>,
    pub sandbox_mode: Option<String>,
    pub trust_level: Option<String>,
}

/// Reads the display fields of `config.toml`; `project` selects the `[projects."<path>"]` entry.
pub fn read_config(path: &Path, project: &Path) -> CodexConfig {
    let Ok(doc) = std::fs::read_to_string(path)
        .unwrap_or_default()
        .parse::<toml_edit::DocumentMut>()
    else {
        return CodexConfig::default();
    };
    let text = |item: Option<&toml_edit::Item>| {
        item.and_then(|i| {
            i.as_str()
                .map(str::to_owned)
                .or_else(|| i.is_table_like().then(|| "granular".to_owned()))
        })
    };
    let trust = doc
        .get("projects")
        .and_then(|p| p.get(project.to_string_lossy().as_ref()))
        .and_then(|p| p.get("trust_level"));
    CodexConfig {
        approval_policy: text(doc.get("approval_policy")),
        sandbox_mode: text(doc.get("sandbox_mode")),
        trust_level: text(trust),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "approval_policy = \"on-request\"\nsandbox_mode = \"workspace-write\"\n[projects.\"/w/p\"]\ntrust_level = \"trusted\"\n",
        )
        .unwrap();
        let config = read_config(&path, Path::new("/w/p"));
        assert_eq!(config.approval_policy.as_deref(), Some("on-request"));
        assert_eq!(config.sandbox_mode.as_deref(), Some("workspace-write"));
        assert_eq!(config.trust_level.as_deref(), Some("trusted"));
        assert_eq!(read_config(&path, Path::new("/other")).trust_level, None);
    }
}
