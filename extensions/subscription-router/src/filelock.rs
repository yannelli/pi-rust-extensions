//! Cross-process lock as pi-ai `store_lock` (and `proper-lockfile`): a `<file>.lock` directory,
//! touched while held; a directory older than the stale period is taken over.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use tokio::task::JoinHandle;

/// Stale period, and the longest wait for another holder.
pub const STALE: Duration = Duration::from_secs(30);

/// A held lock. Dropping it removes the lock directory.
#[derive(Debug)]
pub struct FileLock {
    dir: PathBuf,
    keepalive: Option<JoinHandle<()>>,
}

pub fn lock_dir(file: &Path) -> PathBuf {
    let mut name = file
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".lock");
    file.with_file_name(name)
}

fn is_stale(dir: &Path, stale: Duration) -> bool {
    match std::fs::metadata(dir).and_then(|m| m.modified()) {
        Ok(modified) => SystemTime::now()
            .duration_since(modified)
            .is_ok_and(|age| age > stale),
        Err(error) => error.kind() == ErrorKind::NotFound,
    }
}

fn touch(dir: &Path) {
    if let Ok(handle) = std::fs::File::open(dir) {
        let _ = handle.set_modified(SystemTime::now());
    }
}

impl FileLock {
    /// One attempt. `Ok(None)` means another holder has a fresh lock.
    pub fn try_acquire(file: &Path, stale: Duration) -> std::io::Result<Option<Self>> {
        let dir = lock_dir(file);
        if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
            crate::fsutil::create_private_dir(parent)?;
        }
        let held = |dir: PathBuf| {
            Some(Self {
                dir,
                keepalive: None,
            })
        };
        match std::fs::create_dir(&dir) {
            Ok(()) => Ok(held(dir)),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {
                if !is_stale(&dir, stale) {
                    return Ok(None);
                }
                let _ = std::fs::remove_dir(&dir);
                match std::fs::create_dir(&dir) {
                    Ok(()) => Ok(held(dir)),
                    Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(None),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Retries with backoff for up to [`STALE`], then fails with `WouldBlock`.
    pub async fn acquire(file: &Path) -> std::io::Result<Self> {
        let deadline = tokio::time::Instant::now() + STALE;
        let mut retry = 0u32;
        loop {
            if let Some(mut lock) = Self::try_acquire(file, STALE)? {
                lock.start_keepalive();
                return Ok(lock);
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let message = format!("{} is locked by another process", file.display());
                return Err(std::io::Error::new(ErrorKind::WouldBlock, message));
            }
            let base = (10u64 << retry.min(16)).min(1000);
            let jitter = u64::from(rand::random::<u32>()) % base;
            retry += 1;
            tokio::time::sleep(Duration::from_millis(base + jitter).min(remaining)).await;
        }
    }

    fn start_keepalive(&mut self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let dir = self.dir.clone();
        self.keepalive = Some(runtime.spawn(async move {
            loop {
                tokio::time::sleep(STALE / 2).await;
                touch(&dir);
            }
        }));
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        if let Some(task) = &self.keepalive {
            task.abort();
        }
        let _ = std::fs::remove_dir(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_is_exclusive_and_stale_locks_are_taken_over() {
        let dir = std::env::temp_dir().join(format!("sr-lock-{}", std::process::id()));
        let file = dir.join("accounts.json");
        assert_eq!(lock_dir(&file), dir.join("accounts.json.lock"));
        let held = FileLock::try_acquire(&file, STALE).unwrap().unwrap();
        assert!(FileLock::try_acquire(&file, STALE).unwrap().is_none());
        std::thread::sleep(Duration::from_millis(5));
        let taken = FileLock::try_acquire(&file, Duration::from_millis(1)).unwrap();
        assert!(taken.is_some_and(|lock| lock.dir.exists()));
        drop(held);
        assert!(!lock_dir(&file).exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
