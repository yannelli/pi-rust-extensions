//! Local wall-clock time without a time zone database: the UTC offset comes from `date +%z`
//! once per process.

use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static OFFSET_SECONDS: OnceLock<i64> = OnceLock::new();

/// `+0130` or `-0800` as seconds.
pub fn parse_offset(text: &str) -> Option<i64> {
    let text = text.trim();
    let (sign, digits) = match text.split_at_checked(1)? {
        ("+", rest) => (1, rest),
        ("-", rest) => (-1, rest),
        _ => return None,
    };
    if digits.len() != 4 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i64 = digits[..2].parse().ok()?;
    let minutes: i64 = digits[2..].parse().ok()?;
    Some(sign * (hours * 3600 + minutes * 60))
}

fn offset() -> i64 {
    *OFFSET_SECONDS.get_or_init(|| {
        std::process::Command::new("date")
            .arg("+%z")
            .output()
            .ok()
            .and_then(|out| parse_offset(&String::from_utf8_lossy(&out.stdout)))
            .unwrap_or(0)
    })
}

pub fn hhmm(unix_seconds: i64, offset: i64) -> String {
    let minutes = (unix_seconds + offset).rem_euclid(86_400) / 60;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

pub fn local_hhmm() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    hhmm(now, offset())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_and_times() {
        assert_eq!(parse_offset("+0130\n"), Some(5400));
        assert_eq!(parse_offset("-0800"), Some(-28_800));
        assert_eq!(parse_offset("UTC"), None);
        assert_eq!(hhmm(0, 0), "00:00");
        assert_eq!(hhmm(3600 * 13 + 60 * 7, -3600), "12:07");
        assert_eq!(hhmm(60, -3600), "23:01");
    }
}
