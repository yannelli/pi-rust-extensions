use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use pi_ext_sdk::Key;

use crate::command::parse_bg_args;
use crate::panel::{Action, View};
use crate::registry::{Registry, StartOptions};
use crate::task::{self, Status, Task, resolve_in};
use crate::tools::{RunArgs, normalize_max_bytes};
use crate::ui;

pub struct Temp(pub PathBuf);

impl Temp {
    pub fn new() -> Self {
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("pi-bgt-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn options(command: &str) -> StartOptions {
    StartOptions {
        command: command.into(),
        notify_on_completion: true,
        trigger_on_completion: true,
        survive_reload: true,
        ..StartOptions::default()
    }
}

pub fn sample(id: &str, status: Status) -> Task {
    Task {
        id: id.into(),
        name: format!("task {id}"),
        command: "true".into(),
        description: None,
        cwd: "/".into(),
        is_agent: false,
        pid: None,
        dir: PathBuf::from("/nonexistent").join(id),
        started_at: 1_000,
        ended_at: None,
        timeout_seconds: None,
        notify_on_completion: true,
        trigger_on_completion: true,
        survive_reload: true,
        status,
        exit_code: None,
        notified: false,
        error: None,
    }
}

pub async fn wait_for(registry: &Registry, id: &str, done: impl Fn(&Task) -> bool) -> Task {
    for _ in 0..100 {
        registry.poll(true).await;
        let task = registry.resolve(id).unwrap();
        if done(&task) {
            return task;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "task {id} did not reach the expected state: {:?}",
        registry.resolve(id)
    );
}

#[test]
fn parses_bg_arguments() {
    let args = parse_bg_args(r#"--name "Unit tests" --agent cargo test -- --nocapture"#).unwrap();
    assert_eq!(args.name.as_deref(), Some("Unit tests"));
    assert!(args.is_agent && args.survive_reload);
    assert_eq!(args.command, "cargo test -- --nocapture");
    let args = parse_bg_args("--no-survive-reload --name dev npm run dev").unwrap();
    assert_eq!(
        (args.name.as_deref(), args.survive_reload),
        (Some("dev"), false)
    );
    assert_eq!(args.command, "npm run dev");
    assert_eq!(parse_bg_args("-- --name x").unwrap().command, "--name x");
    assert!(parse_bg_args("--name 'open").is_err());
    assert!(parse_bg_args("--agent").is_err());
}

#[test]
fn resolves_ids_and_prefixes() {
    let tasks = vec![
        sample("abc123", Status::Running),
        sample("abd999", Status::Completed),
    ];
    assert_eq!(resolve_in(&tasks, "abc").unwrap().id, "abc123");
    assert_eq!(resolve_in(&tasks, "abd999").unwrap().id, "abd999");
    assert!(resolve_in(&tasks, "ab").unwrap_err().contains("matches 2"));
    assert!(resolve_in(&tasks, "zz").is_err());
    assert!(resolve_in(&tasks, " ").is_err());
}

#[test]
fn reads_bounded_head_and_tail() {
    let temp = Temp::new();
    let path = temp.0.join("out.log");
    std::fs::write(&path, "0123456789").unwrap();
    let tail = task::read_bounded(&path, 4, true).unwrap();
    assert_eq!(
        (tail.text.as_str(), tail.total, tail.truncated),
        ("6789", 10, true)
    );
    let head = task::read_bounded(&path, 4, false).unwrap();
    assert_eq!(head.text, "0123");
    assert!(!task::read_bounded(&path, 50, true).unwrap().truncated);
    assert_eq!(
        task::read_bounded(&temp.0.join("none"), 4, true)
            .unwrap()
            .total,
        0
    );
    assert_eq!(normalize_max_bytes(None), 50 * 1024);
    assert_eq!(normalize_max_bytes(Some(1e9)), 50 * 1024);
    assert_eq!(normalize_max_bytes(Some(100.0)), 100);
}

#[test]
fn task_state_round_trips_and_finishes_once() {
    let mut task = sample("a1", Status::Running);
    task.timeout_seconds = Some(2);
    assert!(!task.timed_out(2_999) && task.timed_out(3_000));
    let json = serde_json::to_value(&task).unwrap();
    assert_eq!(json["status"], "running");
    assert_eq!(json["timeoutSeconds"], 2);
    assert_eq!(serde_json::from_value::<Task>(json).unwrap(), task);
    assert!(task.finish(Status::Failed, Some(2)));
    assert!(!task.finish(Status::Completed, Some(0)));
    assert_eq!((task.status, task.exit_code), (Status::Failed, Some(2)));
}

#[test]
fn run_args_defaults() {
    let args: RunArgs =
        serde_json::from_str(r#"{"name":"t","command":"ls","isAgent":false,"timeoutSeconds":1.2}"#)
            .unwrap();
    let options = args.into_options();
    assert!(
        options.notify_on_completion && options.trigger_on_completion && options.survive_reload
    );
    assert_eq!(options.timeout_seconds, Some(2));
    let args: RunArgs = serde_json::from_str(
        r#"{"command":"ls","notifyOnCompletion":false,"triggerOnCompletion":true}"#,
    )
    .unwrap();
    assert!(!args.into_options().trigger_on_completion);
}

#[test]
fn status_and_widget_text() {
    assert_eq!(ui::status_text(&[]), None);
    let tasks = vec![
        sample("r1", Status::Running),
        sample("f1", Status::Failed),
        sample("c1", Status::Completed),
    ];
    assert_eq!(
        ui::status_text(&tasks).unwrap(),
        "bg: 1 running, 1 completed, 1 failed"
    );
    let lines = ui::widget_lines(&tasks, 61_000, Some(20));
    assert_eq!(lines.len(), 1);
    assert!(pi_ext_sdk::style::visible_width(&lines[0]) <= 20);
    assert!(
        pi_ext_sdk::style::strip(&ui::widget_lines(&tasks, 61_000, None)[0])
            .contains("task r1 (r1) 1m00s")
    );
}

#[test]
fn panel_keys_need_double_x_to_stop() {
    let tasks = vec![
        sample("r1", Status::Running),
        sample("c1", Status::Completed),
    ];
    let mut view = View::default();
    assert!(matches!(view.key(&Key::Char('x'), &tasks), Action::None));
    assert!(view.message.as_deref().unwrap().contains("again"));
    assert!(matches!(view.key(&Key::Char('x'), &tasks), Action::Stop(id) if id == "r1"));
    view.key(&Key::Down, &tasks);
    view.key(&Key::Down, &tasks);
    assert_eq!(view.selected, 1);
    view.key(&Key::Enter, &tasks);
    assert!(view.logs);
    assert!(matches!(view.key(&Key::Escape, &tasks), Action::None));
    assert!(!view.logs);
    assert!(matches!(
        view.key(&Key::Char('c'), &tasks),
        Action::ClearFinished
    ));
    assert!(matches!(view.key(&Key::Char('q'), &tasks), Action::Close));
    let lines = view.render(&tasks, Some(30));
    assert!(
        lines
            .iter()
            .all(|l| pi_ext_sdk::style::visible_width(l) <= 30)
    );
}
