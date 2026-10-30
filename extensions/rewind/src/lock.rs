//! Cross-process lock on a checkpoint storage directory: one file per lock generation in
//! `<storage>/.pi-checkpoint-lock/`; takers `create_new` the next one, so one taker wins.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use pi_ext_sdk::{Result, bail};
use tokio::task::JoinHandle;

pub const LOCK_DIR: &str = ".pi-checkpoint-lock";
pub const STALE_AFTER: Duration = Duration::from_secs(180);
pub const WAIT: Duration = Duration::from_secs(30);
const RELEASED: &str = "released";
const RETRY: Duration = Duration::from_millis(100);
const ATTEMPTS: usize = 16;

/// Holds one lock generation until dropped. Only the owner writes its generation file.
pub struct StorageLock {
    path: PathBuf,
    refresh: Option<JoinHandle<()>>,
}

enum Current {
    Free(u64),
    Held,
    Retry,
}

impl StorageLock {
    pub async fn acquire(dir: &Path) -> Result<Self> {
        Self::acquire_with(dir, WAIT, STALE_AFTER).await
    }

    /// Waits up to `wait` for the lock; takes over a lock older than `stale`.
    pub async fn acquire_with(dir: &Path, wait: Duration, stale: Duration) -> Result<Self> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(mut lock) = Self::try_acquire(dir, stale)? {
                lock.refresh = Some(tokio::spawn(refresh(lock.path.clone(), stale / 4)));
                return Ok(lock);
            }
            if tokio::time::Instant::now() >= deadline {
                let pid = holder(dir).unwrap_or_else(|| "unknown".into());
                bail!(
                    "checkpoint storage {} is locked by another pi process (pid {pid})",
                    dir.display()
                );
            }
            tokio::time::sleep(RETRY).await;
        }
    }

    /// Takes the lock when it is free or stale; `None` while another holder has it.
    pub fn try_acquire(dir: &Path, stale: Duration) -> io::Result<Option<Self>> {
        let locks = lock_dir(dir);
        create_private_dir(&locks)?;
        for _ in 0..ATTEMPTS {
            let next = match current(&locks, stale)? {
                Current::Free(next) => next,
                Current::Held => return Ok(None),
                Current::Retry => continue,
            };
            if let Some(lock) = Self::claim(dir, next)? {
                return Ok(Some(lock));
            }
        }
        Ok(None)
    }

    /// Creates generation `next` and keeps it only while no newer generation exists, since a
    /// taker that paused after reading the newest generation can claim an obsolete number.
    #[doc(hidden)]
    pub fn claim(dir: &Path, next: u64) -> io::Result<Option<Self>> {
        let locks = lock_dir(dir);
        let path = locks.join(next.to_string());
        match create_new(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => return Ok(None),
            Err(err) => return Err(err),
        }
        if generations(&locks)?.into_iter().any(|g| g > next) {
            let _ = fs::remove_file(&path);
            return Ok(None);
        }
        remove_older(&locks, next);
        Ok(Some(Self {
            path,
            refresh: None,
        }))
    }

    /// Writes a new timestamp; false when a newer holder took over and removed this generation.
    pub fn refresh_now(&self) -> io::Result<bool> {
        overwrite(&self.path, &held_text())
    }
}

impl Drop for StorageLock {
    fn drop(&mut self) {
        if let Some(refresh) = self.refresh.take() {
            refresh.abort();
        }
        let _ = overwrite(&self.path, RELEASED);
    }
}

pub fn lock_dir(storage: &Path) -> PathBuf {
    storage.join(LOCK_DIR)
}

async fn refresh(path: PathBuf, every: Duration) {
    loop {
        tokio::time::sleep(every).await;
        if !matches!(overwrite(&path, &held_text()), Ok(true)) {
            return;
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn held_text() -> String {
    format!("{} {}\n", std::process::id(), now_ms())
}

fn generations(locks: &Path) -> io::Result<Vec<u64>> {
    Ok(fs::read_dir(locks)?
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse().ok())
        .collect())
}

/// The generation to create next, or `Held` while the newest generation is fresh.
fn current(locks: &Path, stale: Duration) -> io::Result<Current> {
    let Some(top) = generations(locks)?.into_iter().max() else {
        return Ok(Current::Free(0));
    };
    let path = locks.join(top.to_string());
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Current::Retry),
        Err(err) => return Err(err),
    };
    if text.trim() == RELEASED || age(&path, &text)? >= stale {
        return Ok(Current::Free(top + 1));
    }
    Ok(Current::Held)
}

/// Age of a held lock from its timestamp, or from the file mtime while the text is partial.
fn age(path: &Path, text: &str) -> io::Result<Duration> {
    let stamp = text.split_whitespace().nth(1).and_then(|t| t.parse().ok());
    let stamp = match stamp {
        Some(ms) => ms,
        None => match fs::metadata(path).and_then(|meta| meta.modified()) {
            Ok(time) => time
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
            Err(err) if err.kind() == io::ErrorKind::NotFound => now_ms(),
            Err(err) => return Err(err),
        },
    };
    Ok(Duration::from_millis(now_ms().saturating_sub(stamp)))
}

fn holder(dir: &Path) -> Option<String> {
    let locks = lock_dir(dir);
    let top = generations(&locks).ok()?.into_iter().max()?;
    let text = fs::read_to_string(locks.join(top.to_string())).ok()?;
    text.split_whitespace().next().map(String::from)
}

fn remove_older(locks: &Path, keep: u64) {
    for generation in generations(locks).unwrap_or_default() {
        if generation < keep {
            let _ = fs::remove_file(locks.join(generation.to_string()));
        }
    }
}

fn create_new(path: &Path) -> io::Result<()> {
    let mut options = fs::File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    options.open(path)?.write_all(held_text().as_bytes())
}

/// Rewrites an existing generation file; false when it no longer exists.
fn overwrite(path: &Path, text: &str) -> io::Result<bool> {
    let file = fs::File::options().write(true).truncate(true).open(path);
    match file {
        Ok(mut file) => file.write_all(text.as_bytes()).map(|()| true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(err),
    }
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}
