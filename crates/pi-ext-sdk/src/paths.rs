//! Config locations. Prefer `Host::agent_dir` and `Host::project_file` once pi sent `initialize`.

use std::path::{Component, Path, PathBuf};

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

/// When `cwd` is in a linked git worktree, the same directory in the main checkout.
/// `None` outside git, in the main checkout, in a submodule, and for a bare repository.
pub fn main_checkout_dir(cwd: impl AsRef<Path>) -> Option<PathBuf> {
    let cwd = cwd.as_ref();
    let root = cwd.ancestors().find(|dir| dir.join(".git").exists())?;
    let text = std::fs::read_to_string(root.join(".git")).ok()?;
    let git_dir = normalize(&root.join(text.trim().strip_prefix("gitdir:")?.trim()));
    let common = std::fs::read_to_string(git_dir.join("commondir")).ok()?;
    let common = normalize(&git_dir.join(common.trim()));
    if common.file_name()? != ".git" {
        return None;
    }
    let main = common.parent()?;
    Some(main.join(cwd.strip_prefix(root).ok()?)).filter(|dir| dir != cwd)
}

/// `<cwd>/<rel>`, or in a linked worktree without it, the main checkout's copy when that exists.
/// Project files that git does not track (local settings, `.pi/ext/*.json`) then apply in
/// every worktree of the repository, and the worktree's own file wins.
pub fn project_path(cwd: impl AsRef<Path>, rel: impl AsRef<Path>) -> PathBuf {
    let (cwd, rel) = (cwd.as_ref(), rel.as_ref());
    let local = cwd.join(rel);
    if local.exists() {
        return local;
    }
    main_checkout_dir(cwd)
        .map(|main| main.join(rel))
        .filter(|path| path.exists())
        .unwrap_or(local)
}

/// `<cwd>/.pi/ext/<name>.json`, with the main checkout's copy as the fallback in a worktree.
pub fn project_ext_config(cwd: impl AsRef<Path>, name: &str) -> PathBuf {
    project_path(
        cwd,
        Path::new(".pi").join("ext").join(format!("{name}.json")),
    )
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
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

    /// `main/.git` plus a linked worktree `wt` with a relative `commondir`, as `git worktree add` writes.
    fn repo_with_worktree() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        let wt = tmp.path().join("wt");
        let admin = main.join(".git/worktrees/wt");
        std::fs::create_dir_all(&admin).unwrap();
        std::fs::create_dir_all(wt.join("sub")).unwrap();
        std::fs::create_dir_all(main.join("sub")).unwrap();
        std::fs::write(admin.join("commondir"), "../..\n").unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", admin.display())).unwrap();
        (tmp, main, wt)
    }

    #[test]
    fn a_worktree_maps_to_the_same_directory_of_the_main_checkout() {
        let (_tmp, main, wt) = repo_with_worktree();
        assert_eq!(main_checkout_dir(&wt), Some(main.clone()));
        assert_eq!(main_checkout_dir(wt.join("sub")), Some(main.join("sub")));
        assert_eq!(main_checkout_dir(&main), None);
        let submodule = main.join("vendor/lib");
        std::fs::create_dir_all(main.join(".git/modules/lib")).unwrap();
        std::fs::create_dir_all(&submodule).unwrap();
        std::fs::write(submodule.join(".git"), "gitdir: ../../.git/modules/lib\n").unwrap();
        assert_eq!(main_checkout_dir(&submodule), None);
    }

    #[test]
    fn project_files_fall_back_to_the_main_checkout() {
        let (_tmp, main, wt) = repo_with_worktree();
        let rel = Path::new(".pi/ext/todo.json");
        assert_eq!(project_ext_config(&wt, "todo"), wt.join(rel));
        std::fs::create_dir_all(main.join(".pi/ext")).unwrap();
        std::fs::write(main.join(rel), "{}").unwrap();
        assert_eq!(project_ext_config(&wt, "todo"), main.join(rel));
        std::fs::create_dir_all(wt.join(".pi/ext")).unwrap();
        std::fs::write(wt.join(rel), "{}").unwrap();
        assert_eq!(project_ext_config(&wt, "todo"), wt.join(rel));
    }
}
