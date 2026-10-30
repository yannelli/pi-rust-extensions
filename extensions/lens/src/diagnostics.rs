use std::path::{Component, Path, PathBuf};

use serde_json::{Value, json};

fn absolute(root: &Path, path: &str) -> String {
    let path = Path::new(path);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .to_string_lossy()
    .into_owned()
}

pub const MAX_FINDINGS: usize = 200;
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024;

pub fn finding(path: &Path, source: &str, severity: &str, message: &str) -> Value {
    json!({"path":path, "line":1, "column":1, "source":source, "severity":severity, "message":clip(message, MAX_MESSAGE_BYTES)})
}

/// Cuts `text` to at most `max` bytes on a character boundary.
pub fn clip(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max.saturating_sub(3);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Keeps the first [`MAX_FINDINGS`] findings of one source, clips their messages,
/// and adds one finding that counts the rest.
pub fn cap(mut findings: Vec<Value>, path: &Path, source: &str) -> Vec<Value> {
    let total = findings.len();
    findings.truncate(MAX_FINDINGS);
    for item in &mut findings {
        if let Some(message) = item["message"].as_str()
            && message.len() > MAX_MESSAGE_BYTES
        {
            item["message"] = json!(clip(message, MAX_MESSAGE_BYTES));
        }
    }
    if total > MAX_FINDINGS {
        let rest = format!("{} more findings not shown", total - MAX_FINDINGS);
        findings.push(finding(path, source, "warning", &rest));
    }
    findings
}

pub fn parse(source: &str, output: &str, root: &Path, file: &Path) -> Vec<Value> {
    let mut findings = vec![];
    if let Ok(Value::Array(items)) = serde_json::from_str(output) {
        for item in items {
            if let Some(messages) = item["messages"].as_array() {
                for message in messages {
                    findings.push(json!({
                        "path":absolute(root,item["filePath"].as_str().unwrap_or_default()),
                        "line":message["line"].as_u64().unwrap_or(1),
                        "column":message["column"].as_u64().unwrap_or(1),
                        "severity":if message["severity"] == 2 {"error"} else {"warning"},
                        "message":message["message"], "source":source, "code":message["ruleId"]
                    }));
                }
            } else if item.get("location").is_some() && item.get("message").is_some() {
                findings.push(json!({
                    "path":absolute(root,item["filename"].as_str().unwrap_or_default()),
                    "line":item["location"]["row"], "column":item["location"]["column"],
                    "severity":"warning", "source":source, "message":item["message"], "code":item["code"]
                }));
            } else if item.get("message").is_some() {
                let mut value = item;
                value["source"] = json!(source);
                if value.get("path").is_none() {
                    value["path"] = json!(file);
                }
                if value.get("severity").is_none() {
                    value["severity"] = json!("error");
                }
                findings.push(value);
            }
        }
        return findings;
    }
    for line in output.lines() {
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if value["reason"] == "compiler-message" {
                let message = &value["message"];
                let span = message["spans"]
                    .as_array()
                    .and_then(|spans| spans.iter().find(|span| span["is_primary"] == true));
                let span = span.cloned().unwrap_or_default();
                findings.push(json!({
                    "path":absolute(root,span["file_name"].as_str().unwrap_or_else(|| file.to_str().unwrap_or_default())),
                    "line":span["line_start"].as_u64().unwrap_or(1), "column":span["column_start"].as_u64().unwrap_or(1),
                    "severity":message["level"], "source":source, "message":message["message"], "code":message["code"]["code"]
                }));
                continue;
            }
        }
        if let Some(value) = text_diagnostic(line, source, root) {
            findings.push(value);
        }
    }
    findings
}

fn text_diagnostic(line: &str, source: &str, root: &Path) -> Option<Value> {
    let (location, message) = line.split_once(": ")?;
    let (path, row, column) = if let Some((path, rest)) = location.split_once('(') {
        let rest = rest.trim_end_matches(')');
        let (row, column) = rest.split_once(',')?;
        (path, row.parse::<u64>().ok()?, column.parse::<u64>().ok()?)
    } else {
        let mut parts = location.rsplitn(3, ':');
        let last = parts.next()?;
        let previous = parts.next()?;
        match parts.next() {
            Some(path) if previous.parse::<u64>().is_ok() => {
                (path, previous.parse().ok()?, last.parse().ok()?)
            }
            _ => (location.rsplit_once(':')?.0, last.parse().ok()?, 1),
        }
    };
    let severity = if message.contains("warning") {
        "warning"
    } else {
        "error"
    };
    Some(
        json!({"path":absolute(root,path),"line":row,"column":column,"message":message,"severity":severity,"source":source}),
    )
}

pub fn normalize_path(cwd: &Path, path: &str) -> PathBuf {
    let path = Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    if let Ok(path) = joined.canonicalize() {
        return path;
    }
    if let (Some(parent), Some(name)) = (joined.parent(), joined.file_name()) {
        if let Ok(parent) = parent.canonicalize() {
            return parent.join(name);
        }
    }
    joined
}

/// True when the normalized `path` lies under the resolved `cwd`.
pub fn within(cwd: &Path, path: &Path) -> bool {
    let cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
    !path.components().any(|part| part == Component::ParentDir) && path.starts_with(cwd)
}
