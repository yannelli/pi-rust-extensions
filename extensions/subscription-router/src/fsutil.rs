use std::io::Write;
use std::path::Path;

/// Writes `bytes` to a sibling temp file with mode 0600, then renames it over `path`.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

pub fn write_json(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    write_private(path, text.as_bytes())
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// ISO time without milliseconds, as upstream `isoSeconds`.
pub fn iso_seconds(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| ms.to_string())
}

pub fn iso_minute(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.format("%Y-%m-%dT%H:%MZ").to_string())
        .unwrap_or_else(|| ms.to_string())
}

/// `2d 4h`, `3h 5m`, `12m`, or `now`, as multi-codex `formatDuration`.
pub fn duration(ms: i64) -> String {
    if ms <= 0 {
        return "now".into();
    }
    let minutes = (ms + 59_999) / 60_000;
    let (days, hours, mins) = (minutes / 1440, (minutes % 1440) / 60, minutes % 60);
    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 || (days == 0 && hours == 0) {
        parts.push(format!("{mins}m"));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_durations_and_times() {
        assert_eq!(duration(0), "now");
        assert_eq!(duration(61_000), "2m");
        assert_eq!(duration(26 * 3_600_000 + 60_000), "1d 2h 1m");
        assert_eq!(iso_seconds(1_500), "1970-01-01T00:00:01Z");
        assert_eq!(iso_minute(61_000), "1970-01-01T00:01Z");
    }

    #[test]
    fn writes_private_files() {
        let dir = std::env::temp_dir().join(format!("sr-fs-{}", std::process::id()));
        let path = dir.join("a/b.json");
        write_json(&path, &serde_json::json!({ "a": 1 })).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"a\": 1"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
