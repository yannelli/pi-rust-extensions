use std::path::Path;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use pi_ext_rewind::config::Config;
use pi_ext_rewind::lock::{STALE_AFTER, StorageLock, lock_dir};
use pi_ext_rewind::state::Session;
use pi_ext_rewind::testdir::TestDir;

const SHORT: Duration = Duration::from_millis(300);

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

fn write_generation(storage: &Path, generation: u64, text: &str) {
    let locks = lock_dir(storage);
    std::fs::create_dir_all(&locks).unwrap();
    std::fs::write(locks.join(generation.to_string()), text).unwrap();
}

fn read_generation(storage: &Path, generation: u64) -> Option<String> {
    std::fs::read_to_string(lock_dir(storage).join(generation.to_string())).ok()
}

fn is_free(storage: &Path) -> bool {
    StorageLock::try_acquire(storage, STALE_AFTER)
        .unwrap()
        .is_some()
}

#[tokio::test]
async fn second_holder_waits_then_gets_the_lock() {
    let dir = TestDir::new("lock-contend");
    let first = StorageLock::acquire_with(dir.path(), SHORT, STALE_AFTER)
        .await
        .unwrap();
    let busy = StorageLock::acquire_with(dir.path(), SHORT, STALE_AFTER).await;
    let err = busy.err().unwrap().to_string();
    assert!(err.contains("locked by another pi process"), "{err}");
    assert!(
        err.contains(&format!("pid {}", std::process::id())),
        "{err}"
    );

    let release = tokio::spawn(async move {
        tokio::time::sleep(SHORT).await;
        drop(first);
    });
    let started = Instant::now();
    let second = StorageLock::acquire_with(dir.path(), Duration::from_secs(5), STALE_AFTER)
        .await
        .unwrap();
    assert!(started.elapsed() >= SHORT - Duration::from_millis(50));
    release.await.unwrap();
    drop(second);
    assert!(is_free(dir.path()));
}

#[tokio::test]
async fn stale_lock_is_taken_over_and_fresh_lock_is_kept() {
    let dir = TestDir::new("lock-stale");
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    write_generation(dir.path(), 0, &format!("{} {}\n", other.id(), now_ms()));
    let held = StorageLock::acquire_with(dir.path(), SHORT, STALE_AFTER).await;
    let _ = other.kill();
    let _ = other.wait();
    assert!(held.is_err());
    let old = now_ms() - 10 * 60 * 1000;
    write_generation(dir.path(), 0, &format!("99999 {old}\n"));
    let _lock = StorageLock::acquire_with(dir.path(), SHORT, STALE_AFTER)
        .await
        .unwrap();
    assert_eq!(read_generation(dir.path(), 0), None);
    let text = read_generation(dir.path(), 1).unwrap();
    assert!(
        text.starts_with(&format!("{} ", std::process::id())),
        "{text}"
    );
}

#[test]
fn old_holder_cannot_touch_the_new_holder_after_takeover() {
    let dir = TestDir::new("lock-takeover");
    let stale = Duration::from_millis(50);
    let old = StorageLock::try_acquire(dir.path(), stale)
        .unwrap()
        .unwrap();
    std::thread::sleep(stale * 2);
    let new = StorageLock::try_acquire(dir.path(), stale)
        .unwrap()
        .unwrap();
    let held = read_generation(dir.path(), 1).unwrap();
    assert_eq!(read_generation(dir.path(), 0), None);

    assert!(!old.refresh_now().unwrap());
    drop(old);
    assert_eq!(read_generation(dir.path(), 0), None);
    assert_eq!(read_generation(dir.path(), 1).unwrap(), held);
    assert!(!is_free(dir.path()));
    assert!(new.refresh_now().unwrap());
    drop(new);
    assert!(is_free(dir.path()));
}

#[test]
fn one_of_many_stale_takers_wins() {
    let old = now_ms() - 10 * 60 * 1000;
    for (round, text) in (0..40).map(|r| {
        let text = if r % 2 == 0 {
            format!("99999 {old}\n")
        } else {
            "released\n".into()
        };
        (r, text)
    }) {
        let dir = TestDir::new("lock-race");
        write_generation(dir.path(), 7, &text);
        let takers = 8;
        let barrier = Arc::new(Barrier::new(takers));
        let handles: Vec<_> = (0..takers)
            .map(|_| {
                let barrier = barrier.clone();
                let path = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    barrier.wait();
                    StorageLock::try_acquire(&path, STALE_AFTER).unwrap()
                })
            })
            .collect();
        let locks: Vec<StorageLock> = handles
            .into_iter()
            .filter_map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(locks.len(), 1, "round {round}: {} holders", locks.len());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn fresh_lock_of_an_exited_process_is_taken_over() {
    let dir = TestDir::new("lock-exited");
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    gone.wait().unwrap();
    write_generation(dir.path(), 0, &format!("{} {}\n", gone.id(), now_ms()));
    let lock = StorageLock::acquire_with(dir.path(), SHORT, STALE_AFTER).await;
    assert!(lock.is_ok());
}

#[tokio::test]
async fn partial_lock_text_goes_stale_by_mtime() {
    let dir = TestDir::new("lock-mtime");
    write_generation(dir.path(), 0, "");
    assert!(!is_free(dir.path()));
    tokio::time::sleep(Duration::from_millis(50)).await;
    let lock = StorageLock::try_acquire(dir.path(), Duration::from_millis(10)).unwrap();
    assert!(lock.is_some());
}

#[tokio::test]
async fn holder_refreshes_its_timestamp() {
    let dir = TestDir::new("lock-refresh");
    let stale = Duration::from_millis(400);
    let _lock = StorageLock::acquire_with(dir.path(), SHORT, stale)
        .await
        .unwrap();
    tokio::time::sleep(stale * 2).await;
    assert!(
        StorageLock::try_acquire(dir.path(), stale)
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn session_operations_wait_for_another_process() {
    let work = TestDir::new("lock-work");
    let storage = TestDir::new("lock-storage");
    std::fs::write(work.path().join("a.txt"), "one\n").unwrap();
    let mut session = Session::new(Config::default(), storage.path().to_path_buf(), work.path());
    let held = StorageLock::acquire(storage.path()).await.unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(SHORT).await;
        drop(held);
    });
    let started = Instant::now();
    let first = session.snapshot("first", None).await.unwrap();
    assert!(started.elapsed() >= SHORT - Duration::from_millis(50));
    release.await.unwrap();
    assert!(is_free(storage.path()));

    std::fs::write(work.path().join("a.txt"), "two\n").unwrap();
    let held = StorageLock::acquire(storage.path()).await.unwrap();
    let release = tokio::spawn(async move {
        tokio::time::sleep(SHORT).await;
        drop(held);
    });
    let started = Instant::now();
    session.restore(&first).await.unwrap();
    assert!(started.elapsed() >= SHORT - Duration::from_millis(50));
    release.await.unwrap();
    let text = std::fs::read_to_string(work.path().join("a.txt")).unwrap();
    assert_eq!(text, "one\n");
    assert!(is_free(storage.path()));
}

#[test]
fn paused_taker_cannot_claim_an_obsolete_generation() {
    let dir = TestDir::new("lock-aba");
    let first = StorageLock::try_acquire(dir.path(), STALE_AFTER)
        .unwrap()
        .unwrap();
    drop(first);
    let newer = StorageLock::try_acquire(dir.path(), STALE_AFTER)
        .unwrap()
        .unwrap();
    assert_eq!(read_generation(dir.path(), 0), None);
    let held = read_generation(dir.path(), 1).unwrap();

    assert!(StorageLock::claim(dir.path(), 0).unwrap().is_none());
    assert_eq!(read_generation(dir.path(), 0), None);
    assert_eq!(read_generation(dir.path(), 1).unwrap(), held);
    assert!(!is_free(dir.path()));
    drop(newer);
    assert!(is_free(dir.path()));
}

#[tokio::test]
async fn git_lock_files_of_a_killed_run_do_not_block_checkpoints() {
    let work = TestDir::new("lock-git-work");
    let storage = TestDir::new("lock-git-storage");
    std::fs::write(work.path().join("a.txt"), "one\n").unwrap();
    let mut session = Session::new(Config::default(), storage.path().to_path_buf(), work.path());
    let first = session.snapshot("first", None).await.unwrap();
    let git_dir = storage.path().join(".git");
    for lock in ["index.lock", "HEAD.lock"] {
        std::fs::write(git_dir.join(lock), "").unwrap();
    }
    std::fs::write(work.path().join("a.txt"), "two\n").unwrap();
    let second = session.snapshot("second", Some(&first)).await.unwrap();
    assert_ne!(first, second);
    assert!(!git_dir.join("index.lock").exists());
}
