use std::io::Write;
use std::path::{Path, PathBuf};

use crate::state::Goal;

/// Goal files under `goalsRoot`: `<id>.json` while open, `archive/<id>.json` after.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

#[derive(Debug, Default)]
pub struct Listing {
    pub goals: Vec<Goal>,
    pub unreadable: Vec<PathBuf>,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    fn archive_dir(&self) -> PathBuf {
        self.root.join("archive")
    }

    /// Open goals, oldest first, plus files that do not parse.
    pub fn list(&self) -> Listing {
        let mut listing = Listing::default();
        let Ok(dir) = std::fs::read_dir(&self.root) else {
            return listing;
        };
        for entry in dir.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "json") || !path.is_file() {
                continue;
            }
            let goal = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<Goal>(&text).ok());
            match goal {
                Some(goal) => listing.goals.push(goal),
                None => listing.unreadable.push(path),
            }
        }
        listing.goals.sort_by_key(|goal| goal.created_at);
        listing
    }

    pub fn save(&self, goal: &Goal) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(goal)?;
        write_atomic(&self.path(&goal.id), text.as_bytes())
    }

    pub fn archive(&self, goal: &Goal) -> std::io::Result<PathBuf> {
        let target = self.archive_dir().join(format!("{}.json", goal.id));
        let text = serde_json::to_string_pretty(goal)?;
        write_atomic(&target, text.as_bytes())?;
        match std::fs::remove_file(self.path(&goal.id)) {
            Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err),
            _ => {}
        }
        Ok(target)
    }

    /// Moves an unreadable file to `archive/<name>.broken`.
    pub fn quarantine(&self, path: &Path) -> std::io::Result<PathBuf> {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        let target = self
            .archive_dir()
            .join(format!("{}.broken", name.unwrap_or_default()));
        std::fs::create_dir_all(self.archive_dir())?;
        std::fs::rename(path, &target)?;
        Ok(target)
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
    let temp = path.with_file_name(format!(
        ".{}.{}.tmp",
        file_name.unwrap_or_default(),
        std::process::id()
    ));
    let mut file = std::fs::File::create(&temp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)
}
