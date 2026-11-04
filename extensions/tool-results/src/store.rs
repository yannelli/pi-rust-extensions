//! Full results under `<agentDir>/ext/tool-results/sessions/<session>/<id>.txt`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::config::NAME;

/// Session directories older than this are left from a crash and get removed.
const STALE: Duration = Duration::from_secs(7 * 24 * 3600);

pub fn root(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(NAME).join("sessions")
}

fn safe(text: &str) -> String {
    text.chars()
        .map(
            |c| match c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                true => c,
                false => '_',
            },
        )
        .collect()
}

pub fn session_dir(agent_dir: &Path, session: Option<&str>) -> PathBuf {
    root(agent_dir).join(safe(session.unwrap_or("no-session")))
}

/// `tr-` and 8 hex digits of FNV-1a over the tool call id.
pub fn result_id(tool_call_id: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in tool_call_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("tr-{hash:08x}")
}

fn valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[derive(Clone, Debug)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// `id`, or `id-2`, `id-3`, and so on when a file with that name exists.
    pub fn free_id(&self, id: &str) -> String {
        let taken = |name: &str| self.dir.join(format!("{name}.txt")).exists();
        if !taken(id) {
            return id.to_owned();
        }
        (2..)
            .map(|n| format!("{id}-{n}"))
            .find(|name| !taken(name))
            .unwrap_or_default()
    }

    pub fn save(&self, id: &str, text: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        std::fs::write(self.dir.join(format!("{id}.txt")), text)
    }

    pub fn load(&self, id: &str) -> Option<String> {
        valid(id)
            .then(|| std::fs::read_to_string(self.dir.join(format!("{id}.txt"))).ok())
            .flatten()
    }

    pub fn clear(&self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Removes session directories other than `keep` that were not touched for a week.
pub fn prune(agent_dir: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(root(agent_dir)) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let modified = entry.metadata().and_then(|m| m.modified()).unwrap_or(now);
        let old = now.duration_since(modified).unwrap_or_default() > STALE;
        if path != keep && path.is_dir() && old {
            let _ = std::fs::remove_dir_all(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_loads_and_clears() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(session_dir(dir.path(), Some("s/1")));
        assert!(store.dir.ends_with("ext/tool-results/sessions/s_1"));
        assert_eq!(store.free_id("tr-1"), "tr-1");
        store.save("tr-1", "one").unwrap();
        assert_eq!(store.free_id("tr-1"), "tr-1-2");
        store.save("tr-1-2", "two").unwrap();
        assert_eq!(store.free_id("tr-1"), "tr-1-3");
        assert_eq!(store.load("tr-1").as_deref(), Some("one"));
        assert_eq!(store.load("tr-1-2").as_deref(), Some("two"));
        assert_eq!(store.load("../x"), None);
        assert_eq!(store.load("missing"), None);
        store.clear();
        assert_eq!(store.load("tr-1"), None);
    }

    #[test]
    fn ids_are_stable_and_safe() {
        assert_eq!(result_id("call_1"), result_id("call_1"));
        assert_ne!(result_id("call_1"), result_id("call_2"));
        assert!(valid(&result_id("x")));
        assert_eq!(result_id("x").len(), 11);
    }

    #[test]
    fn prune_keeps_fresh_and_current_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let current = session_dir(dir.path(), Some("now"));
        let other = session_dir(dir.path(), Some("other"));
        std::fs::create_dir_all(&current).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        prune(dir.path(), &current);
        assert!(current.exists() && other.exists());
    }
}
