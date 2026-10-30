//! Config locations. Prefer `Host::agent_dir` and `Host::project_dir` once pi sent `initialize`.

use std::path::{Path, PathBuf};

/// `$PI_CODING_AGENT_DIR` (with `~` expanded), else `~/.pi/agent`.
pub fn agent_dir() -> PathBuf {
    match std::env::var("PI_CODING_AGENT_DIR") {
        Ok(dir) if !dir.is_empty() => expand_tilde(&dir),
        _ => home_dir().join(".pi").join("agent"),
    }
}

/// `<cwd>/.pi`.
pub fn project_dir(cwd: impl AsRef<Path>) -> PathBuf {
    cwd.as_ref().join(".pi")
}

/// `<base>/extensions-data/<name>`: per-extension files under an agent or project dir.
pub fn extension_dir(base: impl AsRef<Path>, name: &str) -> PathBuf {
    base.as_ref().join("extensions-data").join(name)
}

pub fn home_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    home.map(PathBuf::from).unwrap_or_default()
}

pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~") {
        Some("") => home_dir(),
        Some(rest) if rest.starts_with('/') || rest.starts_with('\\') => {
            home_dir().join(&rest[1..])
        }
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_paths() {
        assert_eq!(project_dir("/work"), PathBuf::from("/work/.pi"));
        assert_eq!(
            extension_dir("/work/.pi", "todo"),
            PathBuf::from("/work/.pi/extensions-data/todo")
        );
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("~/x"), home_dir().join("x"));
    }
}
