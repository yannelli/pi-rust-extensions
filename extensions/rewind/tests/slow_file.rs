//! A file git cannot store within the add timeout is skipped once and stays out of snapshots.

use std::path::Path;
use std::time::Duration;

use pi_ext_rewind::config::Config;
use pi_ext_rewind::state::Session;
use pi_ext_rewind::testdir::TestDir;

/// Incompressible bytes, so git spends real time compressing them.
fn write_noise(path: &Path, bytes: usize) {
    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut data = Vec::with_capacity(bytes);
    while data.len() < bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
    }
    std::fs::write(path, data).unwrap();
}

#[tokio::test]
async fn a_file_that_outlasts_the_git_timeout_is_skipped_for_good() {
    let work = TestDir::new("slow-work");
    let state = TestDir::new("slow-state");
    std::fs::write(work.path().join("small.txt"), "one\n").unwrap();
    write_noise(&work.path().join("huge.bin"), 256 << 20);
    let config = Config {
        git_timeout: Duration::from_secs(1),
        ..Config::default()
    };
    let mut session = Session::new(config, state.path().to_path_buf(), work.path());
    let first = session.snapshot("first", None).await.unwrap();
    let shadow = session.repo.clone();
    let names = |commit: String| {
        let repo = shadow.clone();
        async move {
            repo.git(&["ls-tree", "-r", "--name-only", &commit])
                .await
                .unwrap()
        }
    };
    assert_eq!(names(first.clone()).await.trim(), "small.txt");
    let skip_file = state.path().join(".git/info/pi-rewind-skip");
    assert_eq!(std::fs::read_to_string(&skip_file).unwrap(), "/huge.bin\n");

    std::fs::write(work.path().join("small.txt"), "two\n").unwrap();
    let second = session.snapshot("second", Some(&first)).await.unwrap();
    assert_ne!(first, second);
    assert_eq!(names(second.clone()).await.trim(), "small.txt");
    assert_eq!(std::fs::read_to_string(&skip_file).unwrap(), "/huge.bin\n");

    session.restore(&first).await.unwrap();
    let text = std::fs::read_to_string(work.path().join("small.txt")).unwrap();
    assert_eq!(text, "one\n");
    let huge = std::fs::metadata(work.path().join("huge.bin")).unwrap();
    assert_eq!(huge.len(), 256 << 20);
}
