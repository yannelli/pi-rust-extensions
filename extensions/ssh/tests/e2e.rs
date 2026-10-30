use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn ssh_profiles_tool_runs_under_pi() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["ssh"])
        .turns([
            Turn::tool_call("ssh_profiles", json!({})),
            Turn::text("No profiles are configured."),
        ])
        .prompt("List the configured SSH profiles.")
        .run()
        .unwrap();
    assert_eq!(
        output.tool_result_text("ssh_profiles").as_deref(),
        Some("[]")
    );
    assert!(output.tool_names(0).iter().any(|name| name == "ssh_exec"));
    assert!(output.tool_names(0).iter().any(|name| name == "ssh_read"));
    assert!(output.tool_names(0).iter().any(|name| name == "ssh_write"));
}

#[cfg(unix)]
#[test]
fn ssh_exec_and_file_tools_use_ssh_under_pi() {
    use std::os::unix::fs::PermissionsExt;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};
    if pi_bin_or_skip().is_none() {
        return;
    }
    let id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let directory = std::env::temp_dir().join(format!("ssh-e2e-{id}"));
    std::fs::create_dir_all(&directory).unwrap();
    let ssh = directory.join("ssh");
    std::fs::write(
        &ssh,
        "#!/bin/sh\nfor arg in \"$@\"; do remote=\"$arg\"; done\nexec sh -c \"$remote\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&ssh, std::fs::Permissions::from_mode(0o700)).unwrap();
    let profile = json!({"profiles":{"local":{"host":"fake-host", "cwd":directory}}});
    let setup = format!(
        "mkdir -p \"$PI_CODING_AGENT_DIR/ext\"; printf '%s' '{}' > \"$PI_CODING_AGENT_DIR/ext/ssh.json\"",
        profile
    );
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["ssh"])
        .env(
            "PATH",
            format!("{}:{}", directory.display(), std::env::var("PATH").unwrap()),
        )
        .turns([
            Turn::tool_call("bash", json!({"command":setup})),
            Turn::tool_call(
                "ssh_write",
                json!({"profile":"local","path":"file 'one'", "content":"hello; $HOME\n"}),
            ),
            Turn::tool_call("ssh_read", json!({"profile":"local","path":"file 'one'"})),
            Turn::tool_call(
                "ssh_exec",
                json!({"profile":"local","command":"printf streamed"}),
            ),
            Turn::text("done"),
        ])
        .timeout(Duration::from_secs(30))
        .prompt("Prepare the SSH profile, write and read a file, then execute the command.")
        .run()
        .unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(
        output.tool_result_text("ssh_read").as_deref(),
        Some("hello; $HOME\n"),
        "{}",
        output.stdout
    );
    assert_eq!(
        output.tool_result_text("ssh_exec").as_deref(),
        Some("streamed")
    );
    assert!(
        output
            .tool_updates("ssh_exec")
            .iter()
            .any(|text| text.contains("streamed"))
    );
    assert_eq!(output.tool_results("ssh_write")[0]["isError"], false);
}
