//! Durable results: `<agentDir>/ext/tool-results/store/<id>.txt` and `<id>.json`, keyed by
//! result id alone, so resumed, forked, and restarted sessions find them.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::NAME;

pub fn root(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(NAME).join("store")
}

pub fn hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

/// `tr-` and 12 hex digits of SHA-256 over the tool call id and the content hash.
pub fn result_id(tool_call_id: &str, content_hash: &str) -> String {
    let digest = Sha256::digest(format!("{tool_call_id}\0{content_hash}").as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("tr-{hex}")
}

pub fn valid(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// What `<id>.json` records next to the text.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Meta {
    pub id: String,
    pub tool: String,
    pub tool_call_id: String,
    pub session_id: Option<String>,
    pub session_file: Option<String>,
    pub chars: usize,
    pub lines: usize,
    pub hash: String,
    pub created: u64,
    /// File size of the text in bytes.
    pub bytes: u64,
}

static TEMP: AtomicU64 = AtomicU64::new(0);

/// Writes `path` through a temporary file in the same directory and a rename, so a reader or
/// a crash never sees half a file. `fill` writes the content.
pub fn write_atomic_with(
    path: &Path,
    fill: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let n = TEMP.fetch_add(1, Ordering::Relaxed);
    let temp = dir.join(format!(".{name}.{}.{n}.tmp", std::process::id()));
    let written = std::fs::File::create(&temp).and_then(|mut file| {
        fill(&mut file)?;
        file.sync_all()
    });
    let result = written.and_then(|()| std::fs::rename(&temp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_with(path, |file| file.write_all(bytes))
}

#[derive(Clone, Debug)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn text_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.txt"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.json"))
    }

    /// Writes the text, then the record; a result counts as stored once its record exists.
    pub fn save(&self, meta: &Meta, text: &str) -> std::io::Result<()> {
        write_atomic(&self.text_path(&meta.id), text.as_bytes())?;
        let meta = Meta {
            bytes: text.len() as u64,
            ..meta.clone()
        };
        let json = serde_json::to_vec_pretty(&meta).unwrap_or_default();
        write_atomic(&self.meta_path(&meta.id), &json)
    }

    pub fn load(&self, id: &str) -> Option<String> {
        if !valid(id) || !self.meta_path(id).is_file() {
            return None;
        }
        std::fs::read_to_string(self.text_path(id)).ok()
    }

    pub fn meta(&self, id: &str) -> Option<Meta> {
        let text = valid(id)
            .then(|| std::fs::read(self.meta_path(id)).ok())
            .flatten()?;
        serde_json::from_slice(&text).ok()
    }

    pub fn remove(&self, id: &str) -> bool {
        if !valid(id) {
            return false;
        }
        let meta = std::fs::remove_file(self.meta_path(id)).is_ok();
        let text = std::fs::remove_file(self.text_path(id)).is_ok();
        meta || text
    }

    /// Every record, oldest first.
    pub fn list(&self) -> Vec<Meta> {
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut out: Vec<Meta> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let id = name.strip_suffix(".json")?;
                self.meta(id)
            })
            .collect();
        out.sort_by(|a, b| (a.created, &a.id).cmp(&(b.created, &b.id)));
        out
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
