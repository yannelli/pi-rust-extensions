#![cfg(unix)]

use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use crate::{command, process, remote, tests::Temp};

fn local(script: &str) -> Command {
    let mut command = Command::new("sh");
    command.args(["-c", &remote::supervised(script)]);
    command
}

#[tokio::test]
async fn dash_supervisor_preserves_command_status() {
    if !std::path::Path::new("/bin/dash").exists() {
        return;
    }
    let mut command = Command::new("/bin/dash");
    command.args([
        "-c",
        remote::SUPERVISOR,
        "sh",
        "cat; printf err >&2; exit 7",
    ]);
    let output = process::run_remote(
        command,
        Some("input".into()),
        5,
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(output.stdout, "input");
    assert_eq!(output.stderr, "err");
    assert_eq!(output.code, Some(7));
}

#[tokio::test]
async fn supervised_commands_preserve_output_input_and_exit_code() {
    let input = "hello\n'世界';$HOME\n".repeat(100);
    let output = process::run_remote(
        local("cat; printf err >&2; exit 7"),
        Some(input.clone()),
        5,
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert_eq!(output.stdout, input);
    assert_eq!(output.stderr, "err");
    assert_eq!(output.code, Some(7));
}

#[tokio::test]
async fn watcher_cleanup_does_not_write_to_command_stderr() {
    use std::os::unix::fs::PermissionsExt;

    let temp = Temp::new();
    let cat = temp.0.join("cat");
    let ready = temp.0.join("ready");
    std::fs::write(
        &cat,
        "#!/bin/sh\nprintf watcher-diagnostic >&2\nprintf ready > \"$SSH_WATCHER_READY\"\nexec /bin/cat \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&cat, std::fs::Permissions::from_mode(0o700)).unwrap();
    let script = format!(
        "while [ ! -f {} ]; do sleep 0.01; done; printf out; printf err >&2; exit 7",
        command::quote(ready.to_str().unwrap())
    );
    let mut command = local(&script);
    command.env("SSH_WATCHER_READY", &ready).env(
        "PATH",
        format!(
            "{}:{}",
            temp.0.display(),
            std::env::var("PATH").unwrap_or_default()
        ),
    );
    let output = process::run_remote(command, None, 5, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(
        (output.stdout.as_str(), output.stderr.as_str()),
        ("out", "err")
    );
    assert_eq!(output.code, Some(7));
}

#[tokio::test]
async fn timeout_and_cancel_stop_remote_grandchildren() {
    for cancel in [false, true] {
        let temp = Temp::new();
        let pid_file = temp.0.join("pid");
        let script = format!(
            "sh -c 'trap \"\" TERM; sleep 30 & echo $! > {}; wait' & wait",
            command::quote(pid_file.to_str().unwrap())
        );
        let token = CancellationToken::new();
        let stop = token.clone();
        let runner = tokio::spawn(process::run_remote(local(&script), None, 1, token, None));
        tokio::time::timeout(Duration::from_secs(2), async {
            while !pid_file.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let pid = std::fs::read_to_string(pid_file).unwrap();
        if cancel {
            stop.cancel();
        }
        let error = runner.await.unwrap().err().unwrap();
        assert!(
            error
                .message()
                .contains(if cancel { "cancelled" } else { "timed out" })
        );
        let status = Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .unwrap();
        assert!(!status.success(), "remote child {pid} survived");
    }
}

#[tokio::test]
async fn closing_stdin_stops_remote_command() {
    let temp = Temp::new();
    let pid_file = temp.0.join("pid");
    let script = format!(
        "echo $$ > {}; exec sleep 30",
        command::quote(pid_file.to_str().unwrap())
    );
    let mut child = local(&script)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"0\n").await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while !pid_file.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(stdin);
    tokio::time::timeout(Duration::from_secs(2), child.wait())
        .await
        .unwrap()
        .unwrap();
    let pid = std::fs::read_to_string(pid_file).unwrap();
    assert!(
        !Command::new("kill")
            .args(["-0", pid.trim()])
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn remote_bash_pipe_keeps_stdin_open_and_returns_status() {
    let script = remote::heartbeat_pipe(&remote::supervised("sleep 0.1; printf done; exit 7"));
    let mut command = Command::new("sh");
    command.args(["-c", &script]);
    let output = process::run(command, None, 5, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(output.stdout, "done");
    assert_eq!(output.code, Some(7));
}
