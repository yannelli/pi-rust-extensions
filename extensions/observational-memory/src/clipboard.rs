use std::collections::hash_map::RandomState;
use std::fs::OpenOptions;
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use pi_ext_sdk::{ExecRequest, Host};

use crate::host_glue::state_dir;

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

/// Each command reads `file` through a shell redirect. Its output goes to the null device,
/// so a forked helper that keeps running does not hold the exec pipes open.
pub fn exec_request(platform: &str, command: &str, file: &Path) -> ExecRequest {
    let request = if platform == "win32" {
        let line = format!("{command} < \"{}\" > NUL 2>&1", file.display());
        ExecRequest::new("cmd", &["/C", line.as_str()])
    } else {
        let line = format!("{command} < {} > /dev/null 2>&1", shell_quote(file));
        ExecRequest::new("sh", &["-c", line.as_str()])
    };
    request.timeout_ms(2_000)
}

/// Writes `text` to a new file at `path`, mode 0600 on unix; fails when `path` exists.
pub fn create_private(path: &Path, text: &str) -> std::io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    if let Err(error) = file.write_all(text.as_bytes()) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

/// Writes `text` to `dir/clipboard-<random>.txt` with [`create_private`].
pub fn write_clipboard_file(dir: &Path, text: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let mut hasher = RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    hasher.write_u128(nanos);
    hasher.write_u32(std::process::id());
    let file = dir.join(format!("clipboard-{:016x}.txt", hasher.finish()));
    create_private(&file, text)?;
    Ok(file)
}

/// Copies `text` with the first clipboard command that succeeds.
pub async fn copy_text(host: &Host, text: &str) -> bool {
    let platform = host
        .init()
        .map(|init| init.platform)
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| std::env::consts::OS.replace("macos", "darwin"));
    let Ok(file) = write_clipboard_file(&state_dir(&host.agent_dir()), text) else {
        return false;
    };
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
