use std::path::{Path, PathBuf};

/// Config roots outside pi. `main()` reads them from the environment; tests build them directly.
#[derive(Debug, Clone)]
pub struct Paths {
    pub claude_dir: PathBuf,
    pub codex_home: PathBuf,
    /// Codex system layer: `/etc/codex` on Unix, `%ProgramData%\OpenAI\Codex` on Windows.
    pub codex_system: Option<PathBuf>,
    /// Claude Code managed settings dir (`managed-settings.json` and `managed-settings.d`).
    pub claude_policy: Option<PathBuf>,
    pub temp_dir: PathBuf,
    /// Overrides the agent dir from `initialize`.
    pub agent_dir: Option<PathBuf>,
}

impl Paths {
    pub fn from_env() -> Self {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let var = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty());
        Self {
            claude_dir: var("CLAUDE_CONFIG_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude")),
            codex_home: var("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex")),
            codex_system: codex_system_dir(),
            claude_policy: Some(claude_policy_dir()),
            temp_dir: std::env::temp_dir(),
            agent_dir: None,
        }
    }
}

fn claude_policy_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/ClaudeCode")
    } else if cfg!(windows) {
        PathBuf::from(r"C:\Program Files\ClaudeCode")
    } else {
        PathBuf::from("/etc/claude-code")
    }
}

fn codex_system_dir() -> Option<PathBuf> {
    if cfg!(windows) {
        let data = std::env::var_os("ProgramData").filter(|v| !v.is_empty());
        let data = data.map_or_else(|| PathBuf::from(r"C:\ProgramData"), PathBuf::from);
        Some(data.join("OpenAI").join("Codex"))
    } else {
        Some(PathBuf::from("/etc/codex"))
    }
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

pub fn state_dir(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("hooks")
}

pub fn env_file(agent_dir: &Path, session_id: &str) -> PathBuf {
    let name = if session_id.is_empty() {
        "default"
    } else {
        session_id
    };
    state_dir(agent_dir).join("env").join(format!("{name}.sh"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_root_walks_to_git() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(project_root(&nested, ".claude"), nested);
        std::fs::create_dir(dir.path().join(".git")).unwrap();
        assert_eq!(project_root(&nested, ".claude"), dir.path());
        std::fs::create_dir(nested.join(".claude")).unwrap();
        assert_eq!(project_root(&nested, ".claude"), nested);
    }
}
