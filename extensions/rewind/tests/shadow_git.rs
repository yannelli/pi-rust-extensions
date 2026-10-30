use std::path::Path;
use std::process::Command;

use pi_ext_rewind::config::Config;
use pi_ext_rewind::git::ShadowRepo;
use pi_ext_rewind::testdir::TestDir;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env_remove("GIT_DIR")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn read(dir: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(dir.join(path)).ok()
}

#[tokio::test]
async fn snapshots_and_restores_without_touching_user_git() {
    let work = TestDir::new("work");
    let state = TestDir::new("state");
    let cwd = work.path();
    git(cwd, &["init", "-q"]);
    git(cwd, &["config", "user.email", "t@t"]);
    git(cwd, &["config", "user.name", "t"]);
    write(cwd, ".gitignore", "secret.txt\n");
    write(cwd, "a.txt", "one\n");
    git(cwd, &["add", "."]);
    git(cwd, &["commit", "-qm", "init"]);
    write(cwd, "staged.txt", "staged\n");
    git(cwd, &["add", "staged.txt"]);
    let user_head = git(cwd, &["rev-parse", "HEAD"]);
    let user_index = git(cwd, &["ls-files", "--stage"]);

    write(cwd, "secret.txt", "keep\n");
    write(cwd, "node_modules/x.js", "dep\n");
    write(cwd, "libs/inner/.git/HEAD", "ref: refs/heads/main\n");
    write(cwd, "libs/inner/file.txt", "nested\n");
    let user_status = git(cwd, &["status", "--porcelain"]);
    let config = Config::default();
    let repo = ShadowRepo::new(state.path(), cwd);
    repo.ensure().await.unwrap();
    let before = repo
        .snapshot("before", None, &config.exclude, None)
        .await
        .unwrap();
    let same = repo
        .snapshot("same", Some(&before), &config.exclude, None)
        .await
        .unwrap();
    assert_eq!(before, same, "unchanged tree reuses the commit");

    write(cwd, "a.txt", "one\ntwo\n");
    write(cwd, "new.txt", "new\n");
    std::fs::remove_file(cwd.join("staged.txt")).unwrap();
    write(cwd, "libs/inner/file.txt", "nested changed\n");
    let after = repo
        .snapshot("after", Some(&before), &config.exclude, None)
        .await
        .unwrap();
    assert_ne!(before, after);
    let mut paths: Vec<String> = repo
        .numstat(&before, &after)
        .await
        .unwrap()
        .into_iter()
        .map(|change| change.path)
        .collect();
    paths.sort();
    assert_eq!(paths, ["a.txt", "new.txt", "staged.txt"]);

    repo.ensure().await.unwrap();
    repo.stage_all(&config.exclude, None).await.unwrap();
    repo.checkout(&before).await.unwrap();
    assert_eq!(read(cwd, "a.txt").as_deref(), Some("one\n"));
    assert_eq!(read(cwd, "new.txt"), None);
    assert_eq!(read(cwd, "staged.txt").as_deref(), Some("staged\n"));
    assert_eq!(read(cwd, "secret.txt").as_deref(), Some("keep\n"));
    assert_eq!(read(cwd, "node_modules/x.js").as_deref(), Some("dep\n"));
    assert_eq!(
        read(cwd, "libs/inner/file.txt").as_deref(),
        Some("nested changed\n")
    );

    assert_eq!(git(cwd, &["rev-parse", "HEAD"]), user_head);
    assert_eq!(git(cwd, &["ls-files", "--stage"]), user_index);
    assert_eq!(git(cwd, &["status", "--porcelain"]), user_status);
    assert!(
        git(cwd, &["for-each-ref"])
            .lines()
            .all(|line| line.contains("refs/heads/"))
    );

    repo.checkout(&after).await.unwrap();
    assert_eq!(read(cwd, "new.txt").as_deref(), Some("new\n"));
    assert!(
        repo.checkout("0000000000000000000000000000000000000000")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn skips_oversized_files_and_keeps_them_on_restore() {
    let work = TestDir::new("big");
    let state = TestDir::new("big-state");
    let cwd = work.path();
    let config = Config::default();
    let repo = ShadowRepo::new(state.path(), cwd);
    repo.ensure().await.unwrap();
    write(cwd, "small.txt", "s\n");
    let before = repo
        .snapshot("b", None, &config.exclude, Some(10))
        .await
        .unwrap();
    write(cwd, "big.bin", &"x".repeat(100));
    write(cwd, "small.txt", "s2\n");
    let after = repo
        .snapshot("a", Some(&before), &config.exclude, Some(10))
        .await
        .unwrap();
    let changes = repo.numstat(&before, &after).await.unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, "small.txt");
    repo.stage_all(&config.exclude, Some(10)).await.unwrap();
    repo.checkout(&before).await.unwrap();
    assert_eq!(read(cwd, "small.txt").as_deref(), Some("s\n"));
    assert_eq!(read(cwd, "big.bin").map(|t| t.len()), Some(100));
}
