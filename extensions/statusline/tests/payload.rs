mod support;

use support::*;

#[test]
fn codex_items_map_to_segments() {
    let toml = "model = \"gpt\" # main\n[tui]\nnotifications = true\nstatus_line = [\n  \"model-with-reasoning\", # model\n  'context-remaining', \"current-dir\",\n  \"git-branch\", \"weekly-limit\"\n]\n[mcp]\nstatus_line = [\"nope\"]\n";
    let items = parse_codex_items(toml).expect("items");
    assert_eq!(
        items,
        [
            "model-with-reasoning",
            "context-remaining",
            "current-dir",
            "git-branch",
            "weekly-limit"
        ]
    );
    assert_eq!(
        parse_codex_items("tui.status_line = [\"model\"]").expect("dotted"),
        ["model"]
    );
    assert_eq!(parse_codex_items("[tui]\nanimations = false\n"), None);
    let codex = BuiltinStatusLine::from_codex(&items, 0);
    let line = plain_line(&codex.render(&snapshot(), 120)[0]);
    let cwd = format!(
        "~{}Workspace{}node-cli",
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    );
    assert_eq!(
        line,
        format!("gpt-6.1-sol high · 97% left · {cwd} · format-double-quotes")
    );
}

#[test]
fn reads_claude_and_cursor_commands() {
    let dir = std::env::temp_dir().join(format!("pi-statusline-read-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.json");
    std::fs::write(
        &path,
        r#"{"statusLine": {"type": "command", "command": " ~/.claude/s.sh ", "padding": 1}}"#,
    )
    .unwrap();
    let found = read_command(&path).expect("command");
    assert_eq!(
        (
            found.command.as_str(),
            found.padding,
            found.refresh_interval
        ),
        ("~/.claude/s.sh", Some(1.0), None)
    );
    std::fs::write(
        &path,
        r#"{"statusLine": {"type": "static", "command": "x"}}"#,
    )
    .unwrap();
    assert_eq!(read_command(&path), None);
    assert_eq!(read_command(&dir.join("missing.json")), None);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn colors_accept_tokens_and_hex_and_ink_follows_contrast() {
    let line = first_line(
        json!({"segments": ["dir", "model"], "colors": {"dir": "#ff8800", "model": "accent"}}),
        40,
    );
    assert!(
        line.contains("\x1b[38;2;255;136;0m") || line.contains("\x1b[38;5;"),
        "{line:?}"
    );
    assert!(plain_line(&line).starts_with("node-cli • gpt-6.1-sol"));
    let padded = builtin(json!({"segments": ["dir"], "padding": 3})).render(&snapshot(), 40);
    assert_eq!(plain(&padded), ["   node-cli"]);
    assert_eq!(ink_for([245, 245, 245]), [27, 27, 27]);
    assert_eq!(ink_for([20, 20, 40]), [245, 245, 245]);
    assert!(Item::parse(&json!({"color": "#fff"})).is_none());
}

#[test]
fn payload_uses_claude_code_field_names() {
    let payload = status_payload(&snapshot());
    assert_eq!(payload["hook_event_name"], "Status");
    assert_eq!(payload["cwd"], "/home/me/Workspace/node-cli");
    assert_eq!(payload["workspace"]["current_dir"], payload["cwd"]);
    assert_eq!(payload["model"]["id"], "gpt-6.1-sol");
    assert_eq!(payload["model"]["display_name"], "GPT 6.1 Sol");
    assert_eq!(payload["model"]["provider"], "openai-codex");
    assert_eq!(payload["cost"]["total_cost_usd"], 0.013);
    assert_eq!(payload["context_window"]["context_window_size"], 500_000);
    assert_eq!(payload["context_window"]["used_percentage"], 2.7);
    assert_eq!(payload["context_window"]["remaining_percentage"], 97.3);
    assert_eq!(
        payload["thinking"],
        json!({"enabled": true, "level": "high"})
    );
    assert_eq!(
        payload["git"],
        json!({"branch": "format-double-quotes", "project": "node-cli"})
    );
    assert_eq!(payload["run_state"], "working");
    assert_eq!(payload["tokens"]["cache_read"], 4500);
    assert_eq!(
        payload["transcript_path"],
        "/home/me/.pi/agent/sessions/s.jsonl"
    );
}

#[test]
fn output_lines_drop_trailing_blank_lines() {
    assert_eq!(output_lines("a\tb\r\n\nc\n\n  \n"), ["a    b", "", "c"]);
    assert_eq!(output_lines(&"x\n".repeat(20)).len(), 8);
    assert!(output_lines("").is_empty());
}

#[tokio::test]
async fn command_reads_the_payload_and_sees_the_width() {
    let config = CommandConfig {
        command: r#"set -e; read -r json; printf '%s|%s\n' "$COLUMNS" "${json:0:2}""#.into(),
        padding: 0,
        refresh: None,
    };
    let payload = status_payload(&snapshot()).to_string();
    let lines = run_command(&config, &payload, 77, &std::env::temp_dir())
        .await
        .expect("command ran");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("77|{\""), "{lines:?}");
}

#[tokio::test]
async fn failed_status_commands_do_not_replace_the_last_successful_output() {
    let config = CommandConfig {
        command: "printf 'partial output'; exit 1".into(),
        padding: 0,
        refresh: None,
    };
    assert!(
        run_command(&config, "{}", 80, &std::env::temp_dir())
            .await
            .is_none()
    );
}

#[test]
fn layout_modes_remove_gaps_and_keep_narrow_lines_bounded() {
    let config = json!({"chip": "plain", "icons": "none", "separator": " | ",
        "segments": ["dir", "fill", "model"]});
    let mut compact = config.clone();
    compact["layout"] = json!("compact");
    assert_eq!(
        plain_line(&first_line(compact.clone(), 120)),
        "node-cli | gpt-6.1-sol"
    );
    let mut stacked = config;
    stacked["layout"] = json!("stacked");
    assert_eq!(
        plain(&builtin(stacked.clone()).render(&snapshot(), 120)),
        ["node-cli", "gpt-6.1-sol"]
    );
    for config in [compact, stacked] {
        for width in 0..40 {
            assert!(
                builtin(config.clone())
                    .render(&snapshot(), width)
                    .iter()
                    .all(|line| visible_width(line) <= width)
            );
        }
    }
    let mut no_branch = snapshot();
    no_branch.branch = None;
    let line =
        builtin(json!({"segments": ["branch", "fill", {"text": ""}, "model"], "icons": "none"}));
    assert_eq!(plain(&line.render(&no_branch, 120)), ["gpt-6.1-sol"]);
    assert!(
        builtin(json!({"segments": []}))
            .render(&snapshot(), 120)
            .is_empty()
    );
    assert!(
        builtin(json!({"padding": 3}))
            .render(&snapshot(), 2)
            .is_empty()
    );
}

#[test]
fn context_headroom_and_unknown_usage_are_not_fabricated() {
    let mut data = snapshot();
    assert_eq!(
        segment_text(Segment::ContextTokens, &data).as_deref(),
        Some("14k context")
    );
    assert_eq!(
        segment_text(Segment::ContextHeadroom, &data).as_deref(),
        Some("487k tokens left")
    );
    assert_eq!(
        status_payload(&data)["context_window"]["remaining_tokens"],
        486_500
    );
    data.context.as_mut().unwrap().tokens = Some(600_000);
    assert_eq!(data.context_headroom(), Some(0));
    data.context = None;
    assert_eq!(
        segment_text(Segment::ContextBar, &data).as_deref(),
        Some("context ?")
    );
    assert_eq!(segment_text(Segment::ContextRemaining, &data), None);
    assert_eq!(segment_text(Segment::ContextHeadroom, &data), None);
    assert_eq!(segment_text(Segment::ContextTokens, &data), None);
    assert!(status_payload(&data)["context_window"]["remaining_tokens"].is_null());
}

#[test]
fn statusline_command_switches_presets_and_keeps_the_command() {
    let current = json!({"type": "command", "command": "s.sh", "chip": "dot", "theme": "nord", "segments": ["dir"], "colors": {"dir": "#fff"}});
    let pills = status_line_choice(Some(current), "pills").expect("pills");
    assert_eq!(
        pills,
        json!({"type": "builtin", "preset": "pills", "command": "s.sh", "colors": {"dir": "#fff"}})
    );
    assert_eq!(
        status_line_choice(Some(pills.clone()), "command").expect("command")["type"],
        "command"
    );
    assert_eq!(
        status_line_choice(Some(pills.clone()), "from-claude").expect("claude")["type"],
        "claude"
    );
    assert_eq!(
        status_line_choice(Some(pills.clone()), "claude").expect("claude look")["preset"],
        "claude"
    );
    assert_eq!(
        status_line_choice(Some(pills), "off").expect("off")["type"],
        "off"
    );
    assert!(status_line_choice(None, "command").is_err());
    assert!(
        status_line_choice(None, "rainbow")
            .unwrap_err()
            .contains("retro")
    );
    assert_eq!(
        status_line_choice(None, "from-codex").expect("codex"),
        json!({"type": "codex"})
    );
    assert_eq!(
        status_line_choice(None, "rounded").expect("rounded"),
        json!({"type": "builtin", "preset": "rounded"})
    );

    assert_eq!(status_line_name(None), "default footer");
    assert_eq!(
        status_line_name(Some(&settings(json!({"type": "builtin"})))),
        "minimal"
    );
    assert_eq!(
        status_line_name(Some(&settings(json!({"preset": "powerline"})))),
        "powerline"
    );
    assert_eq!(
        status_line_name(Some(&settings(json!({"type": "cursor"})))),
        "from-cursor"
    );
    assert_eq!(
        status_line_name(Some(&settings(json!({"type": "off"})))),
        "off"
    );
}
