use std::path::{Path, PathBuf};

/// Config roots outside pi. `main()` reads them from the environment; tests build them directly.
#[derive(Debug, Clone)]
pub struct Paths {
    pub home: PathBuf,
    pub claude_dir: PathBuf,
    pub claude_managed: PathBuf,
    pub codex_home: PathBuf,
    /// Codex `requirements.toml` files, highest priority first.
    pub codex_requirements: Vec<PathBuf>,
    /// Overrides the agent dir from `initialize`.
    pub agent_dir: Option<PathBuf>,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
        };
        Self {
            claude_dir: var("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")),
            codex_home: var("CODEX_HOME").unwrap_or_else(|| home.join(".codex")),
            claude_managed: managed_settings(),
            codex_requirements: codex_requirements(),
            agent_dir: None,
            home,
        }
    }
}

/// The system `requirements.toml` and, on Unix, the legacy `managed_config.toml`.
fn codex_requirements() -> Vec<PathBuf> {
    if cfg!(windows) {
        let data = std::env::var_os("ProgramData").unwrap_or_else(|| r"C:\ProgramData".into());
        return vec![PathBuf::from(data).join(r"OpenAI\Codex\requirements.toml")];
    }
    vec![
        PathBuf::from("/etc/codex/requirements.toml"),
        PathBuf::from("/etc/codex/managed_config.toml"),
    ]
}

/// Claude Code managed settings for this platform.
fn managed_settings() -> PathBuf {
    let dir = if cfg!(target_os = "macos") {
        "/Library/Application Support/ClaudeCode"
    } else if cfg!(windows) {
        r"C:\Program Files\ClaudeCode"
    } else {
        "/etc/claude-code"
    };
    Path::new(dir).join("managed-settings.json")
}

/// `cwd` when it holds `marker`, else the nearest ancestor with `.git`, else `cwd`.
pub fn project_root(cwd: &Path, marker: &str) -> PathBuf {
    if cwd.join(marker).exists() {
        return cwd.to_path_buf();
    }
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// `cwd` and its ancestors up to the nearest `.git` directory (or only `cwd` outside git).
pub fn up_to_root(cwd: &Path) -> Vec<PathBuf> {
    let root = project_root(cwd, ".git");
    let dirs: Vec<PathBuf> = cwd.ancestors().map(Path::to_path_buf).collect();
    match dirs.iter().position(|d| *d == root) {
        Some(end) => dirs[..=end].to_vec(),
        None => vec![cwd.to_path_buf()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(project_root(&nested, ".claude"), nested);
        assert_eq!(up_to_root(&nested), vec![nested.clone()]);
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(project_root(&nested, ".claude"), dir.path());
        assert_eq!(up_to_root(&nested).len(), 3);
        std::fs::create_dir(nested.join(".claude")).unwrap();
        assert_eq!(project_root(&nested, ".claude"), nested);
    }
}
