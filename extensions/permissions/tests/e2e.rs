use pi_ext_e2e::{Output, Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn deny_rule_blocks_bash_and_the_model_sees_the_reason() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let claude = tempfile::tempdir().unwrap();
    let settings = json!({ "permissions": { "deny": ["Bash(echo *)"] } });
    std::fs::write(claude.path().join("settings.json"), settings.to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["permissions"])
        .env("CLAUDE_CONFIG_DIR", claude.path().display().to_string())
        .file(
            "agent/ext/permissions.json",
            r#"{"deny": ["Bash(touch *)"]}"#,
        )
        .turns([
            Turn::tool_call("bash", json!({ "command": "echo hi && echo again" })),
            Turn::tool_call("bash", json!({ "command": "touch made.txt" })),
            Turn::text("done"),
        ])
        .prompt("Run echo hi")
        .run()
        .unwrap();
    let results: Vec<String> = output
        .tool_results("bash")
        .iter()
        .map(|e| e["result"]["content"].to_string())
        .collect();
    assert_eq!(results.len(), 2, "stderr: {}", output.stderr);
    assert!(
        results[0].contains("Bash(echo *) from claude-user"),
        "{results:?}"
    );
    assert!(!results[0].contains("again\\n"), "bash ran: {results:?}");
    assert!(
        results[1].contains("Bash(touch *) from pi-global"),
        "{results:?}"
    );
    let last_request = output.requests.last().unwrap().to_string();
    assert!(
        last_request.contains("Blocked by permission rule Bash(echo *)")
            && last_request.contains("Blocked by permission rule Bash(touch *)"),
        "{last_request}"
    );
}

#[test]
fn read_deny_blocks_bash_cat_of_the_file_and_the_model_never_sees_it() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let sentinel = "E2E_SENTINEL_9f3c41d2";
    let claude = tempfile::tempdir().unwrap();
    let settings = json!({ "permissions": { "deny": ["Read(./.env)"] } });
    std::fs::write(claude.path().join("settings.json"), settings.to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["permissions"])
        .env("CLAUDE_CONFIG_DIR", claude.path().display().to_string())
        .file("work/.env", format!("SECRET={sentinel}\n"))
        .turns([
            Turn::tool_call("bash", json!({ "command": "cat .env" })),
            Turn::text("done"),
        ])
        .prompt("Show the env file")
        .run()
        .unwrap();
    let results: Vec<String> = output
        .tool_results("bash")
        .iter()
        .map(|e| e["result"]["content"].to_string())
        .collect();
    assert_eq!(results.len(), 1, "stderr: {}", output.stderr);
    assert!(
        results[0].contains("Read(./.env) from claude-user"),
        "{results:?}"
    );
    assert!(
        !results[0].contains(sentinel),
        "bash read .env: {results:?}"
    );
    let requests: Vec<String> = output.requests.iter().map(ToString::to_string).collect();
    assert_eq!(requests.len(), 2, "stderr: {}", output.stderr);
    assert!(
        requests[1].contains("Blocked by permission rule Read(./.env)"),
        "{}",
        requests[1]
    );
    assert!(
        requests.iter().all(|request| !request.contains(sentinel)),
        "the file content reached the model"
    );
    assert!(!output.stdout.contains(sentinel) && !output.stderr.contains(sentinel));
}

/// A run of the permissions extension with `rules` as `$CODEX_HOME/rules/default.rules`.
fn codex_run(rules: &str) -> (tempfile::TempDir, Run) {
    let codex = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(codex.path().join("rules")).unwrap();
    std::fs::write(codex.path().join("rules/default.rules"), rules).unwrap();
    let run = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["permissions"])
        .env("CODEX_HOME", codex.path().display().to_string());
    (codex, run)
}

fn result_texts(output: &Output, tool: &str) -> Vec<String> {
    output
        .tool_results(tool)
        .iter()
        .map(|e| e["result"]["content"].to_string())
        .collect()
}

const BLOCKED: &str = "Blocked by permission rule";

#[test]
fn codex_forbidden_rule_blocks_rm_and_the_fixture_survives() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let (_codex, run) = codex_run(
        r#"prefix_rule(pattern=["rm", "-rf"], decision="forbidden", justification="e2e keeps its fixture")"#,
    );
    let output = run
        .file("work/keep/data.txt", "fixture-data\n")
        .turns([
            Turn::tool_call("bash", json!({ "command": "rm -rf keep" })),
            Turn::tool_call("bash", json!({ "command": "cat keep/data.txt" })),
            Turn::text("done"),
        ])
        .prompt("Remove keep")
        .run()
        .unwrap();
    let results = result_texts(&output, "bash");
    assert_eq!(results.len(), 2, "stderr: {}", output.stderr);
    assert!(
        results[0].contains(BLOCKED)
            && results[0].contains("from codex-user")
            && results[0].contains("e2e keeps its fixture"),
        "{results:?}"
    );
    assert!(
        results[1].contains("fixture-data"),
        "keep/data.txt is gone: {results:?}"
    );
}

#[test]
fn codex_host_executable_limits_absolute_paths_to_the_listed_ones() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let (_codex, run) = codex_run(
        r#"host_executable(name="git", paths=["/usr/bin/git"])
prefix_rule(pattern=["git", "push"], decision="forbidden", justification="no push in e2e")"#,
    );
    let output = run
        .turns([
            Turn::tool_call(
                "bash",
                json!({ "command": "/usr/bin/git push origin main" }),
            ),
            Turn::tool_call(
                "bash",
                json!({ "command": "/opt/pi-e2e-missing/git push origin main" }),
            ),
            Turn::text("done"),
        ])
        .prompt("Push")
        .run()
        .unwrap();
    let results = result_texts(&output, "bash");
    assert_eq!(results.len(), 2, "stderr: {}", output.stderr);
    assert!(
        results[0].contains(BLOCKED) && results[0].contains("no push in e2e"),
        "{results:?}"
    );
    assert!(
        !results[1].contains(BLOCKED),
        "an unlisted path matched: {results:?}"
    );
}

/// A local `fetch` tool that only writes a marker file named after the URL; it opens no connection.
const FETCH_JS: &str = r#"import { writeFileSync } from "node:fs";
export default function (pi) {
  pi.registerTool({
    name: "fetch",
    label: "fetch",
    description: "Fixture fetch that records the URL.",
    parameters: { type: "object", properties: { url: { type: "string" } }, required: ["url"] },
    execute: async (_id, args) => {
      writeFileSync(`fetched-${args.url.replace(/[^A-Za-z0-9]/g, "_")}.txt`, args.url);
      return { content: [{ type: "text", text: `fetched ${args.url}` }] };
    },
  });
}
"#;

#[test]
fn codex_network_rule_blocks_the_exact_host_before_the_tool_runs() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let (_codex, run) = codex_run(
        r#"network_rule(host="evil.example", protocol="https", decision="deny", justification="no evil hosts")
network_rule(host="ok.example", protocol="https", decision="deny")
network_rule(host="ok.example", protocol="https", decision="allow")
network_rule(host="prompt.example", protocol="https", decision="prompt")"#,
    );
    let fetch = |url: &str| Turn::tool_call("fetch", json!({ "url": url }));
    let output = run
        .file("work/fetch.js", FETCH_JS)
        .pi_args(["-e", "./fetch.js"])
        .turns([
            fetch("https://evil.example/x"),
            fetch("http://EVIL.example:8080/y"),
            fetch("https://api.evil.example/z"),
            fetch("https://ok.example/"),
            fetch("https://prompt.example/"),
            Turn::tool_call("bash", json!({ "command": "ls" })),
            Turn::text("done"),
        ])
        .prompt("Fetch the pages")
        .run()
        .unwrap();
    let results = result_texts(&output, "fetch");
    assert_eq!(results.len(), 5, "stderr: {}", output.stderr);
    for blocked in &results[..2] {
        assert!(
            blocked.contains(BLOCKED) && blocked.contains("no evil hosts"),
            "{results:?}"
        );
    }
    assert!(
        results[2].contains("fetched https://api.evil.example/z"),
        "{results:?}"
    );
    assert!(
        results[3].contains("fetched https://ok.example/"),
        "the later allow did not win over the deny: {results:?}"
    );
    assert!(
        results[4].contains("fetched https://prompt.example/"),
        "a Codex prompt network rule decided the call: {results:?}"
    );
    let listing = result_texts(&output, "bash").concat();
    assert!(
        listing.contains("fetched-https___api_evil_example_z.txt")
            && listing.contains("fetched-https___ok_example_.txt")
            && listing.contains("fetched-https___prompt_example_.txt"),
        "{listing}"
    );
    assert!(
        !listing.contains("fetched-https___evil_example_x.txt")
            && !listing.contains("fetched-http___EVIL_example_8080_y.txt"),
        "a blocked fetch ran its tool body: {listing}"
    );
}

#[test]
fn an_unknown_codex_protocol_rejects_the_file_with_codexs_message() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let (_codex, run) = codex_run(
        r#"prefix_rule(pattern=["echo"], decision="forbidden", justification="never echo")
network_rule(host="evil.example", protocol="ftp", decision="deny")"#,
    );
    let output = run
        .turns([
            Turn::tool_call("bash", json!({ "command": "echo still-runs" })),
            Turn::text("done"),
        ])
        .prompt("Echo")
        .run()
        .unwrap();
    let results = result_texts(&output, "bash");
    assert_eq!(results.len(), 1, "stderr: {}", output.stderr);
    assert!(
        !results[0].contains(BLOCKED) && results[0].contains("still-runs"),
        "the rejected file still applied a rule: {results:?}"
    );
    assert!(
        output.stderr.contains("permissions: ")
            && output.stderr.contains(
                "network_rule protocol must be one of http, https, socks5_tcp, socks5_udp (got ftp)"
            ),
        "{}",
        output.stderr
    );
}

#[test]
fn a_write_to_a_claude_protected_path_is_blocked_without_a_ui_even_with_an_allow_rule() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let claude = tempfile::tempdir().unwrap();
    let settings =
        json!({ "permissions": { "defaultMode": "acceptEdits", "allow": ["Edit(./.claude/**)"] } });
    std::fs::write(claude.path().join("settings.json"), settings.to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["permissions"])
        .env("CLAUDE_CONFIG_DIR", claude.path().display().to_string())
        .turns([
            Turn::tool_call(
                "write",
                json!({ "path": ".claude/settings.json", "content": "{}" }),
            ),
            Turn::tool_call(
                "write",
                json!({ "path": "notes.txt", "content": "e2e-notes-ok" }),
            ),
            Turn::tool_call("read", json!({ "path": "notes.txt" })),
            Turn::text("done"),
        ])
        .prompt("Write the files")
        .run()
        .unwrap();
    let results: Vec<String> = output
        .tool_results("write")
        .iter()
        .map(|e| e["result"]["content"].to_string())
        .collect();
    assert_eq!(results.len(), 2, "stderr: {}", output.stderr);
    assert!(results[0].contains("Claude protected path"), "{results:?}");
    let written = output.tool_results("write")[1];
    assert!(
        written["isError"] != true && written["result"]["isError"] != true,
        "{written}"
    );
    let read = result_texts(&output, "read");
    assert!(
        read.len() == 1 && read[0].contains("e2e-notes-ok"),
        "the ordinary write landed: {read:?}"
    );
}

#[test]
fn a_claude_read_block_refuses_a_read_outside_the_cwd_and_keeps_reads_inside() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let claude = tempfile::tempdir().unwrap();
    let settings = json!({ "permissions": { "blockReadsOutsideWorkingDirectories": true } });
    std::fs::write(claude.path().join("settings.json"), settings.to_string()).unwrap();
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["permissions"])
        .env("CLAUDE_CONFIG_DIR", claude.path().display().to_string())
        .file("agent/outside.txt", "E2E_OUTSIDE_SENTINEL\n")
        .file("work/inside.txt", "E2E_INSIDE_OK\n")
        .turns([
            Turn::tool_call("read", json!({ "path": "../agent/outside.txt" })),
            Turn::tool_call("read", json!({ "path": "inside.txt" })),
            Turn::text("done"),
        ])
        .prompt("Read both files")
        .run()
        .unwrap();
    let results = result_texts(&output, "read");
    assert_eq!(results.len(), 2, "stderr: {}", output.stderr);
    assert!(
        results[0].contains("blockReadsOutsideWorkingDirectories")
            && !results[0].contains("E2E_OUTSIDE_SENTINEL"),
        "{results:?}"
    );
    assert!(results[1].contains("E2E_INSIDE_OK"), "{results:?}");
}
