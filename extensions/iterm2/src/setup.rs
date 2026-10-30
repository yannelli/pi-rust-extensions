//! `/terminal-setup` (after Claude Code): in iTerm2, turns on the integration and lets programs
//! write the clipboard, which `/copy` needs.

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::style;

use crate::features::{self, Features};
use crate::reporter::Reporter;

const DEFAULTS_TIMEOUT: Duration = Duration::from_secs(5);

fn local_mac() -> bool {
    cfg!(target_os = "macos")
        && std::env::var_os("SSH_CONNECTION").is_none()
        && std::env::var_os("SSH_TTY").is_none()
}

pub async fn setup_command(program: &str, args: &[&str]) -> std::result::Result<String, String> {
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let output = tokio::time::timeout(DEFAULTS_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            format!("{program} timed out; grant iTerm2 API access if prompted, then retry")
        })?
        .map_err(|error| format!("could not run {program}: {error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr: String = stderr.trim().chars().take(1000).collect();
        Err(format!("{program} exited with {}: {stderr}", output.status))
    }
}

/// Target this pane, never whichever pane currently has focus.
pub fn iterm_session_id(value: &str) -> Option<&str> {
    let id = value.rsplit(':').next()?;
    (id.starts_with(|c: char| c.is_ascii_alphanumeric())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
    .then_some(id)
}

pub const SESSION_PROPERTIES: [(&str, &str); 4] = [
    ("Option Key Sends", "2"),
    ("Right Option Key Sends", "2"),
    ("BM Growl", "true"),
    ("Send Terminal Generated Alerts", "true"),
];

async fn configure_it2_session(id: &str) -> std::result::Result<(), String> {
    for (property, value) in SESSION_PROPERTIES {
        setup_command(
            "it2",
            &["profile", "session-set-property", id, property, value],
        )
        .await
        .map_err(|error| format!("{property}: {error}. Earlier steps may have applied."))?;
    }
    Ok(())
}

async fn allow_clipboard_access() -> std::result::Result<(), String> {
    if !local_mac() {
        return Err("run /terminal-setup locally on the Mac that runs iTerm2, not over SSH".into());
    }
    let args = [
        "write",
        "com.googlecode.iterm2",
        "AllowClipboardAccess",
        "-bool",
        "true",
    ];
    setup_command("defaults", &args).await.map(|_| ())
}

/// What `/terminal-setup` changed, and the steps that it cannot do.
pub fn setup_report(enabled: bool, clipboard: &std::result::Result<(), String>) -> String {
    let integration = if enabled {
        "- The iTerm2 integration is on (`~/.pi/agent/ext/iterm2.json`): the tab color follows the agent, a badge names the session, and a finished run requests attention."
    } else {
        "- The iTerm2 integration was already on (`~/.pi/agent/ext/iterm2.json` or `terminal.iterm2`), so it did not change."
    };
    let clipboard = match clipboard {
        Ok(()) => "- iTerm2 now lets programs write the clipboard, so `/copy` works. Restart iTerm2 to apply it.".to_string(),
        Err(error) => format!("- Clipboard access did not change: {error}."),
    };
    [
        integration,
        &clipboard,
        "",
        "For optional automation, install [tmc/it2](https://github.com/tmc/it2) on your Mac and run `it2 auth check`. Enable iTerm2 Settings > General > Magic > Enable Python API if required, and approve its access prompt. Run `/terminal-setup` again: Pi offers to set Option-as-Alt and notification alerts for this pane only. It does not install utilities or replace saved profiles.",
        "",
        "Without it2, or to make the settings permanent, follow these steps:",
        "",
        "- To use Option as Alt in key bindings: Settings > Profiles > Keys > General. Set Left Option key and Right Option key to Esc+.",
        "- To see a notification when a run finishes: Settings > Profiles > Terminal. Turn on Notification Center Alerts, click Filter Alerts, and turn on Send escape sequence-generated alerts.",
        "- To show the agent state in the status bar: Settings > Profiles > Session > Configure Status Bar. Add an Interpolated String with `\\(user.pi_state)`. pi also sets `user.pi_session`, `user.pi_project`, and `user.pi_model`.",
        "- In tmux, run `tmux set -g allow-passthrough on` so that the tab color and the badge reach iTerm2.",
    ]
    .join("\n")
}

/// Word-wraps `text` to `width` columns.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            if !line.is_empty()
                && style::visible_width(&line) + 1 + style::visible_width(word) > width
            {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
        .into_iter()
        .map(|line| style::truncate(&line, width))
        .collect()
}

async fn show_report(host: &Host, report: &str) -> Result<()> {
    let render = |width: usize| {
        let mut lines = vec![style::bold("Terminal setup")];
        lines.extend(wrap(report, width));
        lines.push(style::dim("Enter, Esc, or q closes"));
        lines
    };
    let mut panel = host.panel(render(78)).open().await?;
    loop {
        match panel.next_event().await {
            PanelEvent::Key(key) if key.is_cancel() || key.is("enter") => break,
            PanelEvent::Key(_) => {}
            PanelEvent::Resize(width) => panel.set_lines(render(width))?,
            PanelEvent::Closed => return Ok(()),
        }
    }
    panel.close(Value::Null).await.map(|_| ())
}

pub async fn terminal_setup(reporter: Arc<Reporter>, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    if !reporter.terminal().iterm2 {
        let text = "/terminal-setup sets up iTerm2, and this terminal is not iTerm2. Shift+Enter needs no setup in iTerm2, Ghostty, kitty, WezTerm, and Windows Terminal.";
        return host.notify_info(text).await;
    }
    let effective = reporter.config_value(&host).await;
    let path = features::global_config_path(&host.agent_dir());
    let enabled = features::enable(&path, &effective).map_err(|e| Error::msg(e.to_string()))?;
    if enabled {
        reporter.set_features(Features::from_value(Some(&serde_json::json!(true))));
    }
    reporter.report(&host, ctx.snapshot()).await;
    let clipboard = allow_clipboard_access().await;
    show_report(&host, &setup_report(enabled, &clipboard)).await?;
    if !local_mac() {
        return Ok(());
    }
    let id = std::env::var("ITERM_SESSION_ID").ok();
    let Some(id) = id.as_deref().and_then(iterm_session_id) else {
        return Ok(());
    };
    let help = setup_command("it2", &["profile", "session-set-property", "--help"]).await;
    if !help.is_ok_and(|text| text.contains("session-set-property")) {
        let text = "Optional it2 automation unavailable. See Terminal setup for installation and manual steps.";
        return host.notify_info(text).await;
    }
    let question = "Use it2 to set both Option keys to Esc+ and enable escape-sequence notifications? Only this pane changes; saved profiles are untouched.";
    if !host
        .confirm("Configure this iTerm2 pane?", question)
        .await?
    {
        return Ok(());
    }
    match configure_it2_session(id).await {
        Ok(()) => host.notify_info("iTerm2 pane configured: Option-as-Alt and terminal notifications enabled. No restart needed for these pane settings.").await,
        Err(error) => host.notify_error(format!("iTerm2 pane setup failed: {error}")).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_targets_only_the_explicit_iterm_pane() {
        assert_eq!(iterm_session_id("w0t0p0:abc-123"), Some("abc-123"));
        assert_eq!(iterm_session_id("abc-123"), Some("abc-123"));
        assert_eq!(iterm_session_id(""), None);
        assert_eq!(iterm_session_id("w0t0p0:"), None);
        assert_eq!(iterm_session_id("--all"), None);
        assert_eq!(iterm_session_id("id with spaces"), None);
        assert_eq!(SESSION_PROPERTIES[0], ("Option Key Sends", "2"));
        assert_eq!(
            SESSION_PROPERTIES[3],
            ("Send Terminal Generated Alerts", "true")
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn setup_command_reports_failure_instead_of_claiming_success() {
        assert_eq!(
            setup_command("sh", &["-c", "printf ready"]).await.unwrap(),
            "ready"
        );
        let error = setup_command("sh", &["-c", "echo denied >&2; exit 1"])
            .await
            .unwrap_err();
        assert!(error.contains("denied"));
    }

    #[test]
    fn report_says_what_changed() {
        let report = setup_report(true, &Ok(()));
        assert!(report.contains("The iTerm2 integration is on"));
        assert!(report.contains("Restart iTerm2"));
        assert!(report.contains("\\(user.pi_state)"));
        let report = setup_report(false, &Err("no defaults".into()));
        assert!(report.contains("already on"));
        assert!(report.contains("Clipboard access did not change: no defaults."));
    }

    #[test]
    fn wraps_long_lines_to_the_width() {
        let lines = wrap("one two three four five six", 10);
        assert_eq!(lines, ["one two", "three four", "five six"]);
        assert!(
            wrap(&setup_report(true, &Ok(())), 40)
                .iter()
                .all(|l| style::visible_width(l) <= 40)
        );
    }
}
