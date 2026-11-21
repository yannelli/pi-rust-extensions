//! Installed Codex plugin bundles: the active version dir and the content fingerprint.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::model::{HookDef, trust_hash};

const LOCAL_VERSION: &str = "local";

/// `local` when installed, else the highest version (codex-rs/core-plugin-common/src/installed.rs).
pub fn active_version(base: &Path) -> Option<String> {
    let versions: Vec<String> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|v| !matches!(v.as_str(), "" | "." | ".."))
        .filter(|v| {
            v.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.+".contains(c))
        })
        .collect();
    if versions.iter().any(|v| v == LOCAL_VERSION) {
        return Some(LOCAL_VERSION.to_owned());
    }
    versions.into_iter().max_by(|a, b| compare_versions(a, b))
}

/// Semantic version order when both parse, else text order.
fn compare_versions(a: &str, b: &str) -> Ordering {
    match (semver(a), semver(b)) {
        (Some((a, a_pre)), Some((b, b_pre))) => a.cmp(&b).then_with(|| match (a_pre, b_pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => compare_pre(a, b),
        }),
        _ => a.cmp(b),
    }
}

fn semver(text: &str) -> Option<([u64; 3], Option<&str>)> {
    let core = text.split('+').next()?;
    let (core, pre) = match core.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (core, None),
    };
    let numbers: Vec<u64> = core
        .split('.')
        .map(|p| {
            let digits = !p.is_empty() && p.chars().all(|c| c.is_ascii_digit());
            (digits && (p == "0" || !p.starts_with('0')))
                .then(|| p.parse().ok())
                .flatten()
        })
        .collect::<Option<_>>()?;
    Some((numbers.try_into().ok()?, pre))
}

fn compare_pre(a: &str, b: &str) -> Ordering {
    for (x, y) in a.split('.').zip(b.split('.')) {
        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            _ => x.cmp(y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    a.split('.').count().cmp(&b.split('.').count())
}

/// The trust hash of a Codex plugin hook, which covers the fingerprint of its plugin files.
pub fn bundle_trust_hash(def: &HookDef, fingerprint: &str) -> String {
    let file = format!("{}#{fingerprint}", def.file.display());
    trust_hash(&file, &def.event, def.matcher.as_deref(), &def.handler)
}

/// False for a Codex plugin hook whose plugin files changed after the config loaded, as Codex
/// checks before each run. `fingerprints` caches each plugin root for one dispatch.
pub fn bundle_unchanged(
    def: &HookDef,
    fingerprints: &mut HashMap<PathBuf, Option<String>>,
) -> bool {
    let root = def.plugin.as_ref().and_then(|p| p.root.as_deref());
    let Some(root) = root.filter(|_| def.source.is_codex()) else {
        return true;
    };
    let current = fingerprints
        .entry(PathBuf::from(root))
        .or_insert_with(|| fingerprint(Path::new(root)).ok());
    let unchanged = current
        .as_deref()
        .is_some_and(|current| bundle_trust_hash(def, current) == def.hash);
    if !unchanged {
        pi_ext_sdk::info!(
            "hooks: skipping {def}: its plugin files changed; open /hooks to review it"
        );
    }
    unchanged
}

/// The bundle hash of codex-rs/hooks/src/engine/plugin_fingerprint.rs.
pub fn fingerprint(root: &Path) -> std::io::Result<String> {
    let mut pending = vec![root.to_path_buf()];
    let mut hash = Sha256::new();
    hash.update(b"codex-plugin-bundle-v1");
    while let Some(path) = pending.pop() {
        let relative = path.strip_prefix(root).map_err(std::io::Error::other)?;
        let name = relative.file_name().and_then(|n| n.to_str());
        if name.is_some_and(|n| {
            matches!(n, "__pycache__" | ".venv") || n.ends_with(".pyc") || n.ends_with(".egg-info")
        }) {
            continue;
        }
        let bytes = relative.as_os_str().as_encoded_bytes();
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
        let meta = std::fs::symlink_metadata(&path)?;
        if meta.is_dir() {
            hash.update(b"directory");
            let mut entries = std::fs::read_dir(&path)?.collect::<Result<Vec<_>, _>>()?;
            entries.sort_by_key(std::fs::DirEntry::file_name);
            pending.extend(entries.iter().rev().map(std::fs::DirEntry::path));
        } else if meta.is_file() {
            hash.update(b"file");
            hash.update(meta.len().to_le_bytes());
            let copied = std::io::copy(&mut std::fs::File::open(&path)?, &mut hash)?;
            if copied != meta.len() {
                return Err(std::io::Error::other(
                    "plugin file changed while fingerprinting",
                ));
            }
        } else {
            let message = format!("unsupported plugin entry type: {}", path.display());
            return Err(std::io::Error::other(message));
        }
    }
    Ok(format!("sha256:{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_wins_then_the_highest_version() {
        let dir = tempfile::tempdir().unwrap();
        for version in ["1.9.0", "1.10.0", "1.10.0-rc.1", "bad version", "0.9"] {
            std::fs::create_dir_all(dir.path().join(version)).unwrap();
        }
        assert_eq!(active_version(dir.path()).as_deref(), Some("1.10.0"));
        std::fs::create_dir_all(dir.path().join("local")).unwrap();
        assert_eq!(active_version(dir.path()).as_deref(), Some("local"));
        assert_eq!(
            compare_versions("1.0.0-alpha", "1.0.0-alpha.1"),
            Ordering::Less
        );
        assert_eq!(compare_versions("1.0.0-2", "1.0.0-beta"), Ordering::Less);
    }

    #[test]
    fn fingerprint_skips_bytecode_and_changes_with_scripts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("hooks/__pycache__")).unwrap();
        std::fs::write(dir.path().join("hooks/run.sh"), "echo 1").unwrap();
        let first = fingerprint(dir.path()).unwrap();
        std::fs::write(dir.path().join("hooks/__pycache__/x.pyc"), "x").unwrap();
        assert_eq!(fingerprint(dir.path()).unwrap(), first);
        std::fs::write(dir.path().join("hooks/run.sh"), "echo 2").unwrap();
        assert_ne!(fingerprint(dir.path()).unwrap(), first);
        assert!(first.starts_with("sha256:"));
    }
}
