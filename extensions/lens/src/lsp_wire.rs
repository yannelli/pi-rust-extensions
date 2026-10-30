use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::ChildStdin;

use crate::config::LspServer;

pub(super) const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEADER_BYTES: u64 = 8 * 1024;

pub(super) async fn read_message<R: AsyncRead + Unpin>(
    reader: &mut BufReader<R>,
) -> Result<Value, String> {
    let mut length: Option<usize> = None;
    let mut line = Vec::new();
    loop {
        line.clear();
        let read = (&mut *reader)
            .take(MAX_HEADER_BYTES)
            .read_until(b'\n', &mut line)
            .await
            .map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("language server closed its output".into());
        }
        if line.last() != Some(&b'\n') {
            return Err("language server sent a header line without an end".into());
        }
        let header = String::from_utf8_lossy(&line);
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.trim().eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().ok();
        }
    }
    let length = length.ok_or("message without Content-Length")?;
    if length > MAX_MESSAGE_BYTES {
        return Err(format!("message of {length} bytes exceeds limit"));
    }
    let mut body = vec![0; length];
    reader
        .read_exact(&mut body)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_slice(&body).map_err(|e| format!("invalid message: {e}"))
}

pub(super) async fn write_message(writer: &mut ChildStdin, message: &Value) -> Result<(), String> {
    let body = message.to_string();
    let frame = format!("Content-Length: {}\r\n\r\n{body}", body.len());
    writer
        .write_all(frame.as_bytes())
        .await
        .map_err(write_error)?;
    writer.flush().await.map_err(write_error)
}

fn write_error(error: std::io::Error) -> String {
    match error.kind() {
        std::io::ErrorKind::BrokenPipe => format!("language server closed its input: {error}"),
        _ => error.to_string(),
    }
}

pub(super) fn convert(diagnostic: &Value, path: &Path, default_source: &str) -> Value {
    let start = &diagnostic["range"]["start"];
    let severity = match diagnostic["severity"].as_u64() {
        Some(2) => "warning",
        Some(3) => "information",
        Some(4) => "hint",
        _ => "error",
    };
    json!({
        "path": path.to_string_lossy(),
        "line": start["line"].as_u64().unwrap_or(0).saturating_add(1),
        "column": start["character"].as_u64().unwrap_or(0).saturating_add(1),
        "severity": severity,
        "message": diagnostic["message"].as_str().unwrap_or_default(),
        "source": diagnostic["source"].as_str().unwrap_or(default_source),
    })
}

pub(super) fn language_id(server: &LspServer, path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let known = match ext {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        "rb" => "ruby",
        "sh" | "bash" => "shellscript",
        _ => {
            return server
                .languages
                .first()
                .cloned()
                .unwrap_or_else(|| ext.to_string());
        }
    };
    known.to_string()
}

pub(super) fn command_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map_or_else(|| command.to_string(), |n| n.to_string_lossy().into_owned())
}

pub(super) fn file_uri(path: &Path) -> String {
    let raw: Vec<u8> = path
        .as_os_str()
        .as_encoded_bytes()
        .iter()
        .map(|byte| {
            if cfg!(windows) && *byte == b'\\' {
                b'/'
            } else {
                *byte
            }
        })
        .collect();
    let mut uri = String::from(if raw.starts_with(b"/") {
        "file://"
    } else {
        "file:///"
    });
    for byte in raw {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

pub(super) fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = &rest[rest.find('/')?..];
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(value)) => {
                decoded.push(value);
                i += 3;
            }
            (byte, _) => {
                decoded.push(byte);
                i += 1;
            }
        }
    }
    while decoded.len() > 1 && decoded.last() == Some(&b'/') {
        decoded.pop();
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
    }
    #[cfg(not(unix))]
    {
        let text = String::from_utf8(decoded).ok()?;
        Some(PathBuf::from(text.trim_start_matches('/')))
    }
}
