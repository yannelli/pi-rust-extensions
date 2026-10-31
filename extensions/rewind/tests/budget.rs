//! A large work tree is staged in budgeted steps that keep their progress.

use std::path::Path;
use std::time::Instant;

use pi_ext_rewind::Staged;
use pi_ext_rewind::config::Config;
use pi_ext_rewind::git::{ShadowRepo, Taken};
use pi_ext_rewind::state::Session;
use pi_ext_rewind::testdir::TestDir;

fn fill(dir: &Path, count: usize) {
    for i in 0..count {
        let sub = dir.join(format!("d{}", i / 100));
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join(format!("f{i}.txt")), format!("{i}\n")).unwrap();
    }
}

async fn tracked(repo: &ShadowRepo) -> usize {
    let listed = repo.git(&["ls-files", "-z"]).await.unwrap();
    listed.split('\0').filter(|path| !path.is_empty()).count()
}

fn files_below(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|entry| match entry.file_type().unwrap().is_dir() {
            true => files_below(&entry.path()),
            false => 1,
        })
        .sum()
}

#[tokio::test]
async fn a_passed_deadline_keeps_the_staged_batches() {
    let work = TestDir::new("budget-work");
    let state = TestDir::new("budget-state");
    fill(work.path(), 1200);
    let config = Config::default();
    let repo = ShadowRepo::new(state.path(), work.path());
    repo.ensure().await.unwrap();
    let stage = || repo.stage(&config.exclude, None, Some(Instant::now()));
    let first = stage().await.unwrap();
    assert_eq!(
        first,
        Staged::Partial {
            staged: 500,
            remaining: 700
        }
    );
    assert_eq!(tracked(&repo).await, 500);
    let second = stage().await.unwrap();
    assert_eq!(
        second,
        Staged::Partial {
            staged: 500,
            remaining: 200
        }
    );
    assert_eq!(stage().await.unwrap(), Staged::Complete);
    assert_eq!(tracked(&repo).await, 1200);
}

#[tokio::test]
async fn a_budgeted_snapshot_commits_only_a_complete_tree() {
    let work = TestDir::new("budget-commit");
    let state = TestDir::new("budget-commit-state");
    fill(work.path(), 700);
    let config = Config::default();
    let repo = ShadowRepo::new(state.path(), work.path());
    repo.ensure().await.unwrap();
    let take = || repo.snapshot_within("s", None, &config.exclude, None, Instant::now());
    let first = take().await.unwrap();
    assert_eq!(
        first,
        Taken::Partial {
            staged: 500,
            remaining: 200
        }
    );
    let head = repo.git(&["rev-parse", "-q", "--verify", "HEAD"]).await;
    assert!(head.is_err(), "no commit before the tree is complete");
    let Taken::Commit(commit) = take().await.unwrap() else {
        panic!("the second pass completes the tree");
    };
    let names = repo
        .git(&["ls-tree", "-r", "--name-only", &commit])
        .await
        .unwrap();
    assert_eq!(names.lines().count(), 700);
}

#[tokio::test]
async fn restore_after_a_partial_pass_removes_every_newer_file() {
    let work = TestDir::new("budget-restore");
    let state = TestDir::new("budget-restore-state");
    std::fs::write(work.path().join("keep.txt"), "keep\n").unwrap();
    let mut session = Session::new(Config::default(), state.path().to_path_buf(), work.path());
    let first = session.snapshot("first", None).await.unwrap();
    fill(work.path(), 1200);
    let exclude = session.config.exclude.clone();
    let partial = session
        .repo
        .stage(&exclude, None, Some(Instant::now()))
        .await;
    assert!(matches!(partial.unwrap(), Staged::Partial { .. }));
    session.restore(&first).await.unwrap();
    assert_eq!(files_below(work.path()), 1);
    let kept = std::fs::read_to_string(work.path().join("keep.txt")).unwrap();
    assert_eq!(kept, "keep\n");
}
