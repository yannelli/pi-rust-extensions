use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const NAME: &str = "claude-marketplace";

/// Paths the extension reads from the process environment unless a test sets them.
#[derive(Debug, Clone, Default)]
pub struct Paths {
    pub agent_dir: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// The Claude config dir (`$CLAUDE_CONFIG_DIR`, else `<home>/.claude`).
    pub claude_config_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    Project,
}

impl Scope {
    pub const ALL: [Scope; 2] = [Scope::User, Scope::Project];

    pub fn parse(text: &str) -> Result<Scope> {
        match text {
            "user" | "global" => Ok(Scope::User),
            "project" => Ok(Scope::Project),
            other => bail!("unknown scope \"{other}\": use user or project"),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::Project => "project",
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Resolved paths and trust for one operation.
#[derive(Debug, Clone)]
pub struct Env {
    pub agent_dir: PathBuf,
    pub cwd: PathBuf,
    pub trusted: bool,
    pub home: PathBuf,
    pub claude_dir: PathBuf,
}

impl Env {
    pub fn new(paths: &Paths, agent_dir: PathBuf, cwd: PathBuf, trusted: bool) -> Env {
        let home = paths
            .home
            .clone()
            .unwrap_or_else(pi_ext_sdk::paths::home_dir);
        let from_env = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .filter(|dir| dir.is_absolute());
        let claude_dir = paths
            .claude_config_dir
            .clone()
            .or(from_env)
            .unwrap_or_else(|| home.join(".claude"));
        Env {
            agent_dir: paths.agent_dir.clone().unwrap_or(agent_dir),
            cwd,
            trusted,
            home,
            claude_dir,
        }
    }

    pub fn scope_root(&self, scope: Scope) -> PathBuf {
        match scope {
            Scope::User => self.agent_dir.clone(),
            Scope::Project => self.cwd.join(".pi"),
        }
    }

    /// Scopes whose files this session reads: project only when trusted.
    pub fn visible_scopes(&self) -> Vec<Scope> {
        if self.trusted {
            vec![Scope::Project, Scope::User]
        } else {
            vec![Scope::User]
        }
    }

    pub fn require(&self, scope: Scope) -> Result<()> {
        if scope == Scope::Project && !self.trusted {
            bail!(
                "project scope needs a trusted project: it installs hooks and MCP servers. Trust the project in pi first."
            );
        }
        Ok(())
    }

    pub fn dirs(&self, scope: Scope) -> Dirs {
        Dirs::new(&self.scope_root(scope))
    }
}

/// Files and directories of one scope.
#[derive(Debug, Clone)]
pub struct Dirs {
    pub root: PathBuf,
    pub state_dir: PathBuf,
}

impl Dirs {
    pub fn new(root: &Path) -> Dirs {
        Dirs {
            root: root.to_path_buf(),
            state_dir: root.join("ext").join(NAME),
        }
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("ext").join(format!("{NAME}.json"))
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir.join("state.json")
    }

    pub fn sources(&self) -> PathBuf {
        self.state_dir.join("sources")
    }

    pub fn plugin_clone(&self, marketplace: &str, plugin: &str) -> PathBuf {
        self.state_dir
            .join("plugin-clones")
            .join(marketplace)
            .join(plugin)
    }

    pub fn plugin_data(&self, marketplace: &str, plugin: &str) -> PathBuf {
        self.state_dir.join("data").join(marketplace).join(plugin)
    }

    pub fn prompts(&self) -> PathBuf {
        self.state_dir.join("resources").join("prompts")
    }

    pub fn skills(&self) -> PathBuf {
        self.state_dir.join("resources").join("skills")
    }

    pub fn agents(&self) -> PathBuf {
        self.root.join("ext").join("subagents").join("agents")
    }

    pub fn hooks_file(&self) -> PathBuf {
        self.root.join("ext").join("hooks.json")
    }

    pub fn mcp_file(&self) -> PathBuf {
        self.root.join("mcp.json")
    }
}
