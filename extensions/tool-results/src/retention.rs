//! Removes stored results by age and by total size, and leftovers of interrupted writes.

use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::store::{Meta, Store};

/// Temporary files and texts without a record older than this are left from a crash.
const LEFTOVER: Duration = Duration::from_secs(3600);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// 0 keeps results of any age.
    pub max_age_days: u64,
    /// 0 sets no size limit.
    pub max_bytes: u64,
    /// Results whose session file exists skip the age limit; the size limit still applies.
    pub keep_while_session_exists: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub removed: Vec<String>,
    pub kept: usize,
    pub bytes: u64,
}

fn session_alive(meta: &Meta) -> bool {
    meta.session_file
        .as_deref()
        .is_some_and(|file| !file.is_empty() && Path::new(file).is_file())
}

/// Applies `policy` at `now` (seconds since the epoch).
pub fn sweep(store: &Store, policy: Policy, now: u64) -> Swept {
    remove_leftovers(store);
    let mut swept = Swept::default();
    let mut kept: Vec<Meta> = Vec::new();
    for meta in store.list() {
        let old = policy.max_age_days > 0
            && now.saturating_sub(meta.created) > policy.max_age_days * 86_400;
        let exempt = policy.keep_while_session_exists && session_alive(&meta);
        if old && !exempt {
            store.remove(&meta.id);
            swept.removed.push(meta.id);
        } else {
            kept.push(meta);
        }
    }
    let mut total: u64 = kept.iter().map(|m| m.bytes).sum();
    let mut kept = kept.into_iter();
    let mut rest = Vec::new();
    for meta in kept.by_ref() {
        if policy.max_bytes == 0 || total <= policy.max_bytes {
            rest.push(meta);
            break;
        }
        total -= meta.bytes;
        store.remove(&meta.id);
        swept.removed.push(meta.id);
    }
    rest.extend(kept);
    swept.kept = rest.len();
    swept.bytes = total;
    swept
}

fn remove_leftovers(store: &Store) {
    let Ok(entries) = std::fs::read_dir(&store.dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .unwrap_or_default();
        let temp = name.starts_with('.') && name.ends_with(".tmp");
        let orphan = name
            .strip_suffix(".txt")
            .is_some_and(|id| !store.dir.join(format!("{id}.json")).exists());
        if (temp || orphan) && age > LEFTOVER {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(store: &Store, id: &str, created: u64, bytes: usize, session: Option<&str>) {
        let meta = Meta {
            id: id.into(),
            created,
            session_file: session.map(str::to_owned),
            ..Meta::default()
        };
        store.save(&meta, &"x".repeat(bytes)).unwrap();
    }

    #[test]
    fn removes_by_age_unless_the_session_file_exists() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store"));
        let session = dir.path().join("s.jsonl");
        std::fs::write(&session, "{}").unwrap();
        let day = 86_400;
        let now = 100 * day;
        put(&store, "tr-old", now - 40 * day, 10, None);
        put(
            &store,
            "tr-alive",
            now - 40 * day,
            10,
            Some(session.to_str().unwrap()),
        );
        put(
            &store,
            "tr-gone",
            now - 40 * day,
            10,
            Some("/no/such/session.jsonl"),
        );
        put(&store, "tr-new", now - day, 10, None);
        let policy = Policy {
            max_age_days: 30,
            max_bytes: 0,
            keep_while_session_exists: true,
        };
        let swept = sweep(&store, policy, now);
        assert_eq!(swept.removed, ["tr-gone", "tr-old"]);
        assert_eq!(swept.kept, 2);
        assert!(store.load("tr-alive").is_some() && store.load("tr-old").is_none());
        let strict = Policy {
            keep_while_session_exists: false,
            ..policy
        };
        assert_eq!(sweep(&store, strict, now).removed, ["tr-alive"]);
    }

    #[test]
    fn removes_the_oldest_results_over_the_size_limit() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store"));
        for (n, id) in ["tr-a", "tr-b", "tr-c", "tr-d"].iter().enumerate() {
            put(&store, id, 1000 + n as u64, 400, None);
        }
        let policy = Policy {
            max_age_days: 0,
            max_bytes: 1000,
            keep_while_session_exists: true,
        };
        let swept = sweep(&store, policy, 2000);
        assert_eq!(swept.removed, ["tr-a", "tr-b"]);
        assert_eq!((swept.kept, swept.bytes), (2, 800));
        assert_eq!(store.list().len(), 2);
        let none = Policy {
            max_bytes: 0,
            ..policy
        };
        assert!(sweep(&store, none, 2000).removed.is_empty());
    }

    #[test]
    fn fresh_leftovers_stay_until_they_are_old() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().join("store"));
        std::fs::create_dir_all(&store.dir).unwrap();
        std::fs::write(store.dir.join(".tr-x.txt.1.0.tmp"), "half").unwrap();
        std::fs::write(store.dir.join("tr-x.txt"), "no record").unwrap();
        let policy = Policy {
            max_age_days: 30,
            max_bytes: 0,
            keep_while_session_exists: false,
        };
        sweep(&store, policy, 0);
        assert!(
            store.dir.join("tr-x.txt").exists(),
            "a write may still be running"
        );
        assert_eq!(
            store.load("tr-x"),
            None,
            "a text without a record is not stored"
        );
    }
}
