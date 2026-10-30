use std::io::Write;
use std::path::Path;

/// `create_dir_all`, with mode 0700 for the directories it creates on unix.
pub fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(dir)
}

/// Writes `bytes` to a new sibling temp file with mode 0600, then renames it over `path`.
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        create_private_dir(parent)?;
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    let suffix = crate::oauth::random_hex(8);
    let tmp = path.with_file_name(format!(".{name}.{}.{suffix}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&tmp).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    let renamed = written.and_then(|()| std::fs::rename(&tmp, path));
    if renamed.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    renamed
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
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("a/b.json");
        let writers: Vec<_> = (0..8)
            .map(|n| {
                let path = path.clone();
                std::thread::spawn(move || write_json(&path, &serde_json::json!({ "a": n })))
            })
            .collect();
        for writer in writers {
            writer.join().unwrap().unwrap();
        }
        write_json(&path, &serde_json::json!({ "a": 1 })).unwrap();
        assert!(std::fs::read_to_string(&path).unwrap().contains("\"a\": 1"));
        let leftovers = std::fs::read_dir(dir.join("a")).unwrap().count();
        assert_eq!(leftovers, 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir.join("a")), 0o700);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
