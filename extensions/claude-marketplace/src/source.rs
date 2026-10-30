use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

pub const OFFICIAL: &str = "claude-plugins-official";
pub const OFFICIAL_REPO: &str = "anthropics/claude-plugins-official";

#[derive(Debug, Clone, PartialEq)]
pub enum MarketplaceSource {
    Git { url: String },
    Local { root: PathBuf, manifest: PathBuf },
}

fn is_url(raw: &str) -> bool {
    ["http://", "https://", "ssh://", "git://", "file://", "git@"]
        .iter()
        .any(|prefix| raw.starts_with(prefix))
}

fn is_owner_repo(raw: &str) -> bool {
    let ok = |part: &str| {
        !part.is_empty()
            && !part.starts_with('.')
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    matches!(raw.split_once('/'), Some((owner, repo)) if ok(owner) && ok(repo) && !repo.contains('/'))
}

pub fn github_url(repo: &str) -> String {
    let repo = repo.trim_end_matches(".git");
    format!("https://github.com/{repo}.git")
}

fn expand(raw: &str, cwd: &Path, home: &Path) -> PathBuf {
    if raw == "~" {
        return home.to_path_buf();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return home.join(rest);
    }
    cwd.join(raw)
}

/// Parses a git URL, GitHub `owner/repo`, a local directory, or a path to `marketplace.json`.
pub fn parse_marketplace(raw: &str, cwd: &Path, home: &Path) -> Result<MarketplaceSource> {
    let raw = raw.trim();
    if raw.is_empty() {
        bail!("missing marketplace source");
    }
    if is_url(raw) {
        return Ok(MarketplaceSource::Git {
            url: raw.to_owned(),
        });
    }
    let path = expand(raw, cwd, home);
    let looks_local = raw.starts_with(['.', '/', '~']) || path.exists();
    if looks_local {
        return local_marketplace(&path);
    }
    if raw == OFFICIAL {
        return Ok(MarketplaceSource::Git {
            url: github_url(OFFICIAL_REPO),
        });
    }
    if is_owner_repo(raw) {
        return Ok(MarketplaceSource::Git {
            url: github_url(raw),
        });
    }
    bail!("cannot read marketplace source \"{raw}\": use a git URL, owner/repo, or a local path")
}

pub fn local_marketplace(path: &Path) -> Result<MarketplaceSource> {
    let path = path
        .canonicalize()
        .with_context(|| format!("{} does not exist", path.display()))?;
    if path.is_file() {
        let parent = path.parent().unwrap_or(Path::new("/"));
        let root = if parent.file_name().is_some_and(|n| n == ".claude-plugin") {
            parent.parent().unwrap_or(parent)
        } else {
            parent
        };
        return Ok(MarketplaceSource::Local {
            root: root.to_path_buf(),
            manifest: path,
        });
    }
    let manifest = path.join(".claude-plugin").join("marketplace.json");
    if !manifest.is_file() {
        bail!("{} has no .claude-plugin/marketplace.json", path.display());
    }
    Ok(MarketplaceSource::Local {
        root: path,
        manifest,
    })
}

#[derive(Debug, Clone, PartialEq)]
pub enum PluginSource {
    Path(String),
    Git {
        url: String,
        git_ref: Option<String>,
        sha: Option<String>,
        subdir: Option<String>,
    },
    Unsupported(String),
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_owned)
}

pub fn parse_plugin(source: &Value) -> PluginSource {
    if let Some(path) = source.as_str() {
        return PluginSource::Path(path.to_owned());
    }
    let kind = source.get("source").and_then(Value::as_str).unwrap_or("");
    let git = |url: Option<String>, subdir: Option<String>| match url {
        Some(url) => PluginSource::Git {
            url,
            git_ref: text(source, "ref"),
            sha: text(source, "sha"),
            subdir,
        },
        None => PluginSource::Unsupported(format!("{kind} source without a URL")),
    };
    match kind {
        "github" => git(text(source, "repo").map(|r| github_url(&r)), None),
        "url" | "git" => git(text(source, "url"), None),
        "git-subdir" => match text(source, "path") {
            Some(path) => git(text(source, "url"), Some(path)),
            None => PluginSource::Unsupported("git-subdir source without a path".into()),
        },
        "npm" => PluginSource::Unsupported("npm plugin sources are not supported".into()),
        other => PluginSource::Unsupported(format!("unknown plugin source \"{other}\"")),
    }
}

/// Joins a relative path to `root` and rejects absolute paths and paths that leave `root`.
pub fn contained(root: &Path, rel: &str) -> Result<PathBuf, String> {
    if rel.starts_with('~') || Path::new(rel).is_absolute() || rel.starts_with('/') {
        return Err(format!(
            "path \"{rel}\" must be relative to the plugin root"
        ));
    }
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir if parts.pop().is_some() => {}
            _ => return Err(format!("path \"{rel}\" leaves the plugin root")),
        }
    }
    let joined = parts.iter().fold(root.to_path_buf(), |acc, p| acc.join(p));
    let escapes = match (joined.canonicalize(), root.canonicalize()) {
        (Ok(real), Ok(real_root)) => !real.starts_with(&real_root),
        _ => false,
    };
    if escapes {
        return Err(format!("path \"{rel}\" leaves the plugin root"));
    }
    Ok(joined)
}

/// Fails unless `path` stays under `base`, following symlinks in the parts that exist.
pub fn ensure_inside(base: &Path, path: &Path) -> Result<()> {
    let leaves = || anyhow::anyhow!("{} leaves {}", path.display(), base.display());
    let rel = path.strip_prefix(base).map_err(|_| leaves())?;
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(leaves());
    }
    let Ok(real_base) = base.canonicalize() else {
        return Ok(());
    };
    let existing = path
        .ancestors()
        .find(|p| std::fs::symlink_metadata(p).is_ok())
        .unwrap_or(base);
    let real = existing.canonicalize().map_err(|_| leaves())?;
    if !real.starts_with(&real_base) {
        return Err(leaves());
    }
    Ok(())
}

/// Marketplace and plugin names become file names, so they must be plain.
pub fn check_name(kind: &str, name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    if !ok {
        bail!("invalid {kind} name \"{name}\"");
    }
    Ok(())
}
