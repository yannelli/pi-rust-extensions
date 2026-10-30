use super::*;
use crate::credentials::Env;
use crate::testutil::TempDir;

fn ledger(dir: &TempDir, day: &'static str) -> Ledger {
    Ledger::open(
        dir.path().join("usage.json"),
        DEFAULT_USD_PER_MTOK,
        Arc::new(move || day.to_owned()),
    )
}

#[test]
fn counts_persist_across_ledgers() {
    let dir = TempDir::new();
    let first = ledger(&dir, "2026-10-29");
    first.record_start();
    first.record_success(1_000_000, 10, 0.042);
    first.record_start();
    first.record_failure();
    let second = ledger(&dir, "2026-10-29");
    let today = second.today();
    assert_eq!(today.totals.requests_started, 2);
    assert_eq!(today.totals.requests_succeeded, 1);
    assert_eq!(today.totals.requests_failed, 1);
    assert_eq!(today.totals.input_tokens, 1_000_000);
    assert!((today.estimated_usd - 0.042).abs() < 1e-9);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&second.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert_eq!(ledger(&dir, "2026-10-30").today().totals, Totals::default());
}

#[test]
fn caps_block_and_merge_lowest() {
    let dir = TempDir::new();
    let ledger = ledger(&dir, "2026-10-29");
    ledger.record_start();
    let caps = Caps {
        max_requests_per_day: Some(1),
        ..Caps::default()
    };
    let blocked = ledger.blocked(&caps).unwrap();
    assert_eq!(
        (blocked.cap, blocked.used, blocked.limit),
        ("requestsPerDay", 1.0, 1.0)
    );
    assert!(ledger.blocked(&Caps::default()).is_none());
    let env = Env::fixed([
        ("PI_TYPESAFE_MAX_REQUESTS_PER_DAY", "5.9"),
        ("PI_TYPESAFE_MAX_USD_PER_DAY", "-1"),
    ]);
    let from_env = Caps::from_env(&env);
    assert_eq!(from_env.max_requests_per_day, Some(5));
    assert_eq!(from_env.max_usd_per_day, None);
    let merged = Caps {
        max_requests_per_day: Some(3),
        max_usd_per_day: Some(2.0),
        ..Caps::default()
    }
    .lowest(from_env);
    assert_eq!(merged.max_requests_per_day, Some(3));
    assert_eq!(merged.max_usd_per_day, Some(2.0));
}

#[test]
fn busy_lock_keeps_pending_and_stale_lock_is_taken() {
    let dir = TempDir::new();
    let ledger = ledger(&dir, "2026-10-29");
    fs::write(&ledger.lock, "").unwrap();
    let started = Instant::now();
    ledger.record_start();
    assert!(started.elapsed() < LOCK_WAIT + Duration::from_millis(300));
    assert!(!ledger.path.exists());
    assert_eq!(ledger.today().totals.requests_started, 1);
    let old = SystemTime::now() - Duration::from_secs(60);
    fs::File::options()
        .write(true)
        .open(&ledger.lock)
        .unwrap()
        .set_modified(old)
        .unwrap();
    ledger.record_start();
    assert_eq!(
        read_days(&ledger.path).unwrap()["2026-10-29"].requests_started,
        2
    );
    assert!(!ledger.lock.exists());
}

#[test]
fn corrupt_file_restarts_count() {
    let dir = TempDir::new();
    let ledger = ledger(&dir, "2026-10-29");
    fs::write(&ledger.path, "{not json").unwrap();
    ledger.record_start();
    assert_eq!(ledger.today().totals.requests_started, 1);
}
