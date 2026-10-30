use std::path::{Path, PathBuf};
use std::time::Duration;

use pi_ext_sdk::{CancellationToken, Error, Host, Result};
use serde_json::{Value, json};

use crate::backend::{Backend, Kind};
use crate::platform::linux::pick_capturer;
use crate::platform::run::{run, run_ok, which};
use crate::platform::{Os, detect, macos, unsupported_message};
use crate::runtime::{captures_dir, state_dir};

const GRANT: &str = "Grant Accessibility and Screen Recording to the app that runs pi (for example, your terminal) in System Settings > Privacy & Security. Screen Recording lets the agent see windows. Accessibility lets the agent read and operate controls.";
const RECHECK: &str = "Recheck (restarts helper)";
const KINDS: [(&str, &str, &str); 2] = [
    (
        "accessibility",
        "Accessibility",
        "Open Accessibility Settings (missing)",
    ),
    (
        "screenRecording",
        "Screen Recording",
        "Open Screen Recording Settings (missing)",
    ),
];

pub const INPUT_NEEDS_UI: &str = "act_ui needs an interactive session: pi-computer-use asks once per session before it sends mouse and keyboard input to the desktop. Start pi in interactive mode.";
pub const INPUT_DECLINED: &str =
    "Desktop input is not allowed in this session. Call act_ui again to be asked again.";

/// Asks once per session before the first desktop input action; refuses without a UI.
pub async fn confirm_input(host: &Host, allowed: bool, app: &str, window: &str) -> Result<()> {
    if allowed {
        return Ok(());
    }
    if !host.has_ui() {
        return Err(Error::msg(INPUT_NEEDS_UI));
    }
    let message = format!(
        "The agent wants to send mouse and keyboard input to {app} ({window}). Allow desktop input for the rest of this session?"
    );
    if host.confirm("Allow desktop input?", &message).await? {
        Ok(())
    } else {
        Err(Error::msg(INPUT_DECLINED))
    }
}

pub fn non_interactive_error() -> String {
    format!(
        "pi-computer-use setup requires an interactive session. Start pi in interactive mode. {GRANT}"
    )
}

pub fn incomplete_error() -> String {
    format!("pi-computer-use setup is incomplete. {GRANT}")
}

pub fn missing(status: &Value) -> Vec<&'static str> {
    KINDS
        .iter()
        .filter(|(key, _, _)| status[*key] != true)
        .map(|(key, _, _)| *key)
        .collect()
}

pub fn prompt(status: &Value) -> String {
    let state = |key: &str| {
        if status[key] == true {
            "granted"
        } else {
            "missing"
        }
    };
    let summary: Vec<String> = KINDS
        .iter()
        .map(|(key, name, _)| format!("{name}: {}", state(key)))
        .collect();
    [
        "pi-computer-use needs macOS permissions.".to_owned(),
        summary.join("; "),
        String::new(),
        GRANT.to_owned(),
        "Enable the toggle, then choose Recheck. If macOS asks you to quit and reopen the app, do that first.".to_owned(),
    ]
    .join("\n")
}

pub fn options(missing: &[&str]) -> Vec<String> {
    let mut options: Vec<String> = KINDS
        .iter()
        .filter(|(key, _, _)| missing.contains(key))
        .map(|(_, _, open)| (*open).to_owned())
        .collect();
    options.push(RECHECK.to_owned());
    options.push("Cancel".to_owned());
    options
}

/// Checks tools and permissions; prompts through the UI when something is missing.
pub async fn ensure_ready(host: &Host, cancel: &CancellationToken) -> Result<Backend> {
    let scratch = captures_dir(&state_dir(host))?;
    let kind = match detect() {
        Os::Unsupported(os) => return Err(Error::msg(unsupported_message(os))),
        Os::Linux { wayland } => {
            if !which("xdotool") {
                return Err(Error::msg(
                    "pi-computer-use on Linux needs xdotool to list windows and send input. Install xdotool, then retry.",
                ));
            }
            Kind::Linux {
                capturer: pick_capturer(wayland, which),
            }
        }
        Os::Mac => Kind::MacOs {
            helper: ensure_helper(host, &state_dir(host), cancel).await?,
        },
    };
    let backend = Backend {
        kind,
        scratch,
        cancel: cancel.clone(),
    };
    if matches!(backend.kind, Kind::MacOs { .. }) {
        ensure_permissions(host, &backend).await?;
    }
    Ok(backend)
}

async fn ensure_permissions(host: &Host, backend: &Backend) -> Result<()> {
    let mut status = backend.helper("check", json!({})).await?;
    if missing(&status).is_empty() {
        return Ok(());
    }
    if !host.has_ui() {
        return Err(Error::msg(non_interactive_error()));
    }
    let _ = backend.helper("register", json!({})).await;
    loop {
        let gaps = missing(&status);
        if gaps.is_empty() {
            return Ok(());
        }
        let choice = host.select(&prompt(&status), &options(&gaps)).await?;
        let Some(choice) = choice.filter(|c| c != "Cancel") else {
            return Err(Error::msg(incomplete_error()));
        };
        if let Some((key, _, _)) = KINDS.iter().find(|(_, _, open)| *open == choice) {
            backend.helper("openPane", json!({ "kind": key })).await?;
            continue;
        }
        status = backend.helper("check", json!({})).await?;
        let gaps = missing(&status);
        if gaps.is_empty() {
            host.notify_info("pi-computer-use is ready.").await?;
        } else {
            let message = format!(
                "Still missing: {}. Enable the toggle, quit and reopen the app that runs pi if macOS asks, then choose Recheck again.",
                gaps.join(" and ")
            );
            host.notify_warning(message).await?;
        }
    }
}

/// Builds the Swift helper into the state dir one time per source hash.
async fn ensure_helper(host: &Host, state: &Path, cancel: &CancellationToken) -> Result<PathBuf> {
    let dir = macos::helper_dir(state);
    let binary = dir.join(macos::HELPER_NAME);
    if binary.is_file() {
        return Ok(binary);
    }
    let found = run(&macos::find_swiftc(), Duration::from_secs(30), cancel).await;
    if !found.is_ok_and(|output| output.ok()) {
        return Err(Error::msg(missing_swift(host).await));
    }
    std::fs::create_dir_all(&dir)?;
    for (name, text) in macos::SOURCES {
        std::fs::write(dir.join(name), text)?;
    }
    if host.has_ui() {
        let _ = host
            .notify_info(
                "Building the pi-computer-use macOS helper. This takes about a minute one time.",
            )
            .await;
    }
    run_ok(&macos::compile(&dir), Duration::from_secs(600), cancel).await?;
    if !binary.is_file() {
        return Err(Error::msg(format!(
            "The helper build did not write {}",
            binary.display()
        )));
    }
    Ok(binary)
}

async fn missing_swift(host: &Host) -> String {
    let message = "pi-computer-use builds its macOS helper with swiftc from the Xcode Command Line Tools. Install them with `xcode-select --install`, then retry.";
    if !host.has_ui() {
        return message.to_owned();
    }
    let choice = host
        .select(message, &["Install Command Line Tools", "Cancel"])
        .await;
    if choice.ok().flatten().as_deref() == Some("Install Command Line Tools") {
        let cancel = CancellationToken::new();
        let _ = run(&macos::install_tools(), Duration::from_secs(30), &cancel).await;
        return "The Command Line Tools installer started. Finish the install, then retry."
            .to_owned();
    }
    message.to_owned()
}
