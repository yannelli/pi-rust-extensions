use std::path::Path;

use pi_ext_sdk::{ExecRequest, Host};

/// Clipboard commands for `platform` (Node `process.platform` names), tried in order.
pub fn clipboard_commands(platform: &str) -> Vec<&'static str> {
    match platform {
        "darwin" => vec!["pbcopy"],
        "win32" => vec!["clip"],
        _ => vec![
            "wl-copy",
            "xclip -selection clipboard",
            "xsel --clipboard --input",
            "termux-clipboard-set",
        ],
    }
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', r"'\''"))
}

/// `exec` has no stdin, so each command reads `file` through a shell redirect.
pub fn exec_request(platform: &str, command: &str, file: &Path) -> ExecRequest {
    let request = if platform == "win32" {
        let line = format!("{command} < \"{}\"", file.display());
        ExecRequest::new("cmd", &["/C", line.as_str()])
    } else {
        let line = format!("{command} < {}", shell_quote(file));
        ExecRequest::new("sh", &["-c", line.as_str()])
    };
    request.timeout_ms(2_000)
}

/// Copies `text` with the first clipboard command that succeeds.
pub async fn copy_text(host: &Host, text: &str) -> bool {
    let platform = host
        .init()
        .map(|init| init.platform)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| std::env::consts::OS.replace("macos", "darwin"));
    let dir = std::env::temp_dir().join("pi-observational-memory");
    let file = dir.join(format!("clipboard-{}.txt", std::process::id()));
    if std::fs::create_dir_all(&dir)
        .and_then(|_| std::fs::write(&file, text))
        .is_err()
    {
        return false;
    }
    let mut copied = false;
    for command in clipboard_commands(&platform) {
        let output = host
            .exec_with(exec_request(&platform, command, &file))
            .await;
        if output.is_ok_and(|o| o.success()) {
            copied = true;
            break;
        }
    }
    let _ = std::fs::remove_file(&file);
    copied
}
