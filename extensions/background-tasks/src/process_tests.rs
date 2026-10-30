use std::time::Duration;

use crate::registry::Registry;
use crate::task::{Status, session_dir};
use crate::tests::{Temp, options, sample, wait_for};
use crate::{identity, process, ui};

#[tokio::test]
async fn runs_commands_and_records_exit_codes() {
    let temp = Temp::new();
    let registry = Registry::default();
    registry.bind(session_dir(&temp.0, Some("s/1")));
    let cwd = temp.0.display().to_string();
    let ok = registry.start(options("echo hello; pwd"), &cwd).unwrap();
    let bad = registry
        .start(options("echo oops >&2; exit 3"), &cwd)
        .unwrap();
    assert!(
        ok.dir
            .starts_with(temp.0.join("ext/background-tasks/sessions/s_1"))
    );
    let ok = wait_for(&registry, &ok.id, |t| !t.status.is_running()).await;
    let bad = wait_for(&registry, &bad.id, |t| !t.status.is_running()).await;
    assert_eq!((ok.status, ok.exit_code), (Status::Completed, Some(0)));
    assert_eq!((bad.status, bad.exit_code), (Status::Failed, Some(3)));
    let output = std::fs::read_to_string(ok.output_path()).unwrap();
    assert_eq!(output, format!("hello\n{cwd}\n"));
    assert!(
        std::fs::read_to_string(bad.output_path())
            .unwrap()
            .contains("oops")
    );
    let pending = registry.poll(false).await;
    assert_eq!(pending.len(), 2);
    registry.mark_notified(&ok.id);
    assert_eq!(registry.poll(false).await.len(), 1);
    assert!(ui::notification_text(&bad).contains("Exit code: 3"));
    assert_eq!(registry.clear_finished(), 2);
    assert!(!ok.dir.exists());
}

#[tokio::test]
async fn new_registry_recovers_running_and_lost_tasks() {
    let temp = Temp::new();
    let dir = session_dir(&temp.0, Some("s1"));
    let first = Registry::default();
    first.bind(dir.clone());
    let cwd = temp.0.display().to_string();
    let task = first.start(options("sleep 0.5; echo done"), &cwd).unwrap();
    let mut lost = sample("dead0001", Status::Running);
    lost.dir = dir.join(&lost.id);
    std::fs::create_dir_all(&lost.dir).unwrap();
    let mut child = std::process::Command::new("true").spawn().unwrap();
    lost.pid = Some(child.id());
    child.wait().unwrap();
    lost.save().unwrap();
    drop(first);

    let second = Registry::default();
    second.bind(dir);
    assert_eq!(second.running().len(), 2);
    let done = wait_for(&second, &task.id, |t| !t.status.is_running()).await;
    assert_eq!(done.status, Status::Completed);
    let lost = second.resolve("dead").unwrap();
    assert_eq!(lost.status, Status::Lost);
}

#[tokio::test]
async fn timeout_stops_the_task() {
    let temp = Temp::new();
    let registry = Registry::default();
    registry.bind(session_dir(&temp.0, None));
    let mut opts = options("sleep 30");
    opts.timeout_seconds = Some(1);
    let task = registry.start(opts, &temp.0.display().to_string()).unwrap();
    let task = wait_for(&registry, &task.id, |t| !t.status.is_running()).await;
    assert_eq!(task.status, Status::TimedOut);
    for _ in 0..40 {
        if !process::group_alive(task.pid.unwrap()).await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("timed out task still runs");
}

#[tokio::test]
async fn stop_kills_term_ignoring_descendants() {
    let temp = Temp::new();
    let registry = Registry::default();
    registry.bind(session_dir(&temp.0, None));
    let command = "(trap '' TERM; sleep 30) & sleep 30";
    let task = registry
        .start(options(command), &temp.0.display().to_string())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let stopped = registry.stop(&task.id, "test").await.unwrap();
    assert_eq!(stopped.status, Status::Killed);
    assert!(!process::group_alive(task.pid.unwrap()).await);
    assert!(
        registry
            .stop(&task.id, "again")
            .await
            .unwrap_err()
            .contains("not running")
    );
}

#[tokio::test]
async fn shutdown_keeps_reload_survivors_only_on_reload() {
    let temp = Temp::new();
    let registry = Registry::default();
    registry.bind(session_dir(&temp.0, None));
    let cwd = temp.0.display().to_string();
    let keep = registry.start(options("sleep 30"), &cwd).unwrap();
    let mut opts = options("sleep 30");
    opts.survive_reload = false;
    let drop_on_reload = registry.start(opts, &cwd).unwrap();
    assert_eq!(registry.shutdown("reload").await, 1);
    assert_eq!(
        registry.resolve(&drop_on_reload.id).unwrap().status,
        Status::Killed
    );
    assert!(registry.resolve(&keep.id).unwrap().status.is_running());
    assert!(process::group_alive(keep.pid.unwrap()).await);
    assert_eq!(registry.shutdown("quit").await, 1);
    let keep = registry.resolve(&keep.id).unwrap();
    assert_eq!(keep.status, Status::Killed);
    assert!(keep.notified);
}

#[cfg(unix)]
#[tokio::test]
async fn task_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let mode =
        |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
    let temp = Temp::new();
    let registry = Registry::default();
    let sessions = session_dir(&temp.0, Some("s1"));
    registry.bind(sessions.clone());
    let task = registry
        .start(options("echo hi"), &temp.0.display().to_string())
        .unwrap();
    let task = wait_for(&registry, &task.id, |t| !t.status.is_running()).await;
    for dir in [temp.0.join("ext"), sessions, task.dir.clone()] {
        assert_eq!(mode(&dir), 0o700, "{}", dir.display());
    }
    for file in [
        task.output_path(),
        task.exit_path(),
        task.dir.join("task.json"),
    ] {
        assert_eq!(mode(&file), 0o600, "{}", file.display());
    }
}

/// A live process in its own group that a group signal to its pid would kill.
#[cfg(unix)]
fn unrelated_process() -> std::process::Child {
    use std::os::unix::process::CommandExt;
    std::process::Command::new("sleep")
        .arg("30")
        .process_group(0)
        .spawn()
        .unwrap()
}

#[cfg(unix)]
#[tokio::test]
async fn reused_pid_is_never_signaled() {
    let temp = Temp::new();
    let dir = session_dir(&temp.0, Some("s1"));
    let mut child = unrelated_process();
    let pid = child.id();
    assert!(identity::identity(pid).unwrap().is_some());
    let records = [
        ("old00001", Some(1)),
        ("old00002", None),
        ("old00003", None),
    ];
    for (id, timeout) in records {
        let mut task = sample(id, Status::Running);
        task.dir = dir.join(id);
        task.pid = Some(pid);
        task.process_start = (id != "old00002").then(|| "Thu Jan  1 00:00:00 1970".into());
        task.timeout_seconds = timeout;
        std::fs::create_dir_all(&task.dir).unwrap();
        task.save().unwrap();
    }
    let registry = Registry::default();
    registry.bind(dir);
    let err = registry.stop("old00003", "test").await.unwrap_err();
    assert!(err.contains("not the task's process"), "{err}");
    assert_eq!(registry.resolve("old00003").unwrap().status, Status::Lost);
    registry.poll(false).await;
    let timed = registry.resolve("old00001").unwrap();
    assert_eq!(timed.status, Status::Lost);
    assert!(timed.error.unwrap().contains("not the task's process"));
    assert!(registry.resolve("old00002").unwrap().status.is_running());
    registry.poll(true).await;
    assert_eq!(registry.resolve("old00002").unwrap().status, Status::Lost);
    let mut legacy = sample("old00004", Status::Running);
    legacy.dir = registry
        .resolve("old00001")
        .unwrap()
        .dir
        .with_file_name("old00004");
    legacy.pid = Some(pid);
    std::fs::create_dir_all(&legacy.dir).unwrap();
    legacy.save().unwrap();
    let registry = Registry::default();
    registry.bind(legacy.dir.parent().unwrap().to_path_buf());
    assert_eq!(registry.shutdown("quit").await, 0);
    assert_eq!(registry.resolve("old00004").unwrap().status, Status::Lost);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "unrelated process was signaled"
    );
    assert_eq!(
        process::terminate(pid, Some("bogus"), Duration::from_millis(10)).await,
        Err(process::unverified_message(pid))
    );
    assert!(child.try_wait().unwrap().is_none());
    child.kill().unwrap();
    child.wait().unwrap();
}
