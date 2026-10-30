use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
#[cfg(unix)]
use std::time::Duration;
#[cfg(unix)]
use tokio_util::sync::CancellationToken;

pub(crate) struct Temp(pub PathBuf);
impl Temp {
    pub fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "ssh-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub(crate) fn profile() -> config::Profile {
    serde_json::from_value(
        json!({"host":"example.org", "user":"alice", "cwd":"/srv/a'b",
        "env":{"MESSAGE":"a'b;$HOME"}, "identityFile":"/keys/my key", "jumpHost":"bastion"}),
    )
    .unwrap()
}

#[test]
fn merge_preserves_unset_profile_fields() {
    let result = config::merge(
        json!({"profiles":{"dev":{"host":"global", "user":"alice"}}}),
        json!({"profiles":{"dev":{"host":"project"},"other":{"host":"other"}}}),
    )
    .unwrap();
    assert_eq!(result.profiles["dev"].host, "project");
    assert_eq!(result.profiles["dev"].user.as_deref(), Some("alice"));
    assert_eq!(result.profiles["dev"].port, 22);
    assert_eq!(result.control_persist, 600);
    assert_eq!(result.profiles.len(), 2);
}

#[test]
fn invalid_profiles_fail_validation() {
    for value in [
        json!({"host":"-oProxyCommand=bad"}),
        json!({"host":"a\nb"}),
        json!({"host":"valid","port":0}),
        json!({"host":"valid","env":{"X;id":"bad"}}),
        json!({"host":"valid","options":{"ControlPath":"other"}}),
    ] {
        let profile: config::Profile = serde_json::from_value(value).unwrap();
        assert!(config::validate("dev", &profile).is_err());
    }
}

#[test]
fn shared_socket_depends_on_connection_and_not_remote_cwd() {
    let temp = Temp::new();
    let mut profile = profile();
    let first = command::socket_path(&temp.0, &profile).unwrap();
    profile.cwd = Some("/other".into());
    assert_eq!(first, command::socket_path(&temp.0, &profile).unwrap());
    profile.port = 2222;
    assert_ne!(first, command::socket_path(&temp.0, &profile).unwrap());
    let args = command::arguments(&temp.0, &config::Config::default(), &profile).unwrap();
    assert!(args.contains(&"ControlMaster=auto".into()));
    assert!(args.contains(&"ControlPersist=600".into()));
    assert!(args.contains(&"/keys/my key".into()));
    assert!(args.contains(&"bastion".into()));
}

#[cfg(unix)]
#[tokio::test]
async fn remote_command_quotes_cwd_env_and_script() {
    let temp = Temp::new();
    let directory = temp.0.join("a'b space");
    std::fs::create_dir(&directory).unwrap();
    let profile = profile();
    let remote = command::remote_command(
        &profile,
        "printf '%s\\n' \"$PWD\" \"$MESSAGE\"",
        directory.to_str(),
    );
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", &remote]);
    let output = process::run(command, None, 5, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_eq!(output.code, Some(0));
    assert_eq!(
        output.stdout,
        format!("{}\na'b;$HOME\n", directory.display())
    );
    let remote = command::remote_command(
        &profile,
        "printf should-not-run",
        Some("/missing-ssh-test-directory"),
    );
    let mut command = tokio::process::Command::new("sh");
    command.args(["-c", &remote]);
    let output = process::run(command, None, 5, CancellationToken::new(), None)
        .await
        .unwrap();
    assert_ne!(output.code, Some(0));
    assert!(output.stdout.is_empty());
}

#[cfg(unix)]
fn fake_ssh(temp: &Temp, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = temp.0.join("ssh");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    format!(
        "{}:{}",
        temp.0.display(),
        std::env::var("PATH").unwrap_or_default()
    )
}

#[cfg(unix)]
#[tokio::test]
async fn fake_ssh_on_path_receives_arguments_and_stdin() {
    let temp = Temp::new();
    let path = fake_ssh(
        &temp,
        "printf '%s\\n' \"$@\"; cat; printf error >&2; exit 7",
    );
    let config = config::Config::default();
    let profile = profile();
    let mut command = command::build(&temp.0, &config, &profile, "cat > '/remote path'").unwrap();
    command.env("PATH", path);
    let output = process::run(
        command,
        Some("payload;$HOME".into()),
        5,
        CancellationToken::new(),
        None,
    )
    .await
    .unwrap();
    assert!(output.stdout.contains("ControlMaster=auto\n"));
    assert!(output.stdout.contains("/keys/my key\n"));
    assert!(
        output
            .stdout
            .ends_with("cat > '/remote path'\npayload;$HOME")
    );
    assert_eq!(output.stderr, "error");
    assert_eq!(output.code, Some(7));
    assert!(output.result().is_error);
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_and_cancellation_stop_ssh() {
    let temp = Temp::new();
    let path = fake_ssh(&temp, "exec sleep 30");
    for cancelled in [false, true] {
        let mut command =
            command::build(&temp.0, &config::Config::default(), &profile(), "true").unwrap();
        command.env("PATH", &path);
        let token = CancellationToken::new();
        if cancelled {
            token.cancel();
        }
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            process::run(command, None, 1, token, None),
        )
        .await
        .unwrap();
        let error = result.err().unwrap().to_string();
        assert!(
            error.contains(if cancelled { "cancelled" } else { "timed out" }),
            "{error}"
        );
    }
}

#[cfg(unix)]
#[test]
fn config_writes_private_files() {
    use std::os::unix::fs::PermissionsExt;
    let temp = Temp::new();
    let path = temp.0.join("config.json");
    config::save(&path, &json!({"profiles":{}})).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
