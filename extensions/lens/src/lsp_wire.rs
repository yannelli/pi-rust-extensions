use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};

use crate::config::LspServer;

const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

pub(super) async fn read_message(reader: &mut BufReader<ChildStdout>) -> Result<Value, String> {
    let mut length: Option<usize> = None;
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader
            .read_line(&mut line)
            .await
            .map_err(|e| e.to_string())?;
        if read == 0 {
            return Err("language server closed its output".into());
        }
        let header = line.trim_end();
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
        .map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())
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
        "line": start["line"].as_u64().unwrap_or(0) + 1,
        "column": start["character"].as_u64().unwrap_or(0) + 1,
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
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from(if raw.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for byte in raw.bytes() {
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
    let text = String::from_utf8(decoded).ok()?;
    let trimmed = text.trim_end_matches('/');
    let path = if cfg!(windows) {
        trimmed.trim_start_matches('/')
    } else {
        trimmed
    };
    Some(PathBuf::from(path))
}
