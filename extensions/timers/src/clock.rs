//! Time sources, duration and datetime parsing, and formatting.

use chrono::{DateTime, Local, NaiveDate, NaiveDateTime, NaiveTime, TimeZone};
use serde::Deserialize;

pub fn now_ms() -> i64 {
    Local::now().timestamp_millis()
}

pub fn local(ms: i64) -> DateTime<Local> {
    Local
        .timestamp_millis_opt(ms)
        .earliest()
        .unwrap_or_else(Local::now)
}

/// `2026-05-01 14:30:00` in local time.
pub fn format_time(ms: i64) -> String {
    local(ms).format("%Y-%m-%d %H:%M:%S").to_string()
}

/// `1h30m`, `15m`, `45s`, `2d`.
pub fn format_duration(secs: u64) -> String {
    if secs == 0 {
        return "0s".into();
    }
    let units = [(86_400, "d"), (3_600, "h"), (60, "m"), (1, "s")];
    let mut rest = secs;
    let mut text = String::new();
    for (size, unit) in units {
        if rest >= size {
            text.push_str(&format!("{}{unit}", rest / size));
            rest %= size;
        }
    }
    text
}

/// A duration argument: a string such as `15m` or `1h30m`, or a number of minutes.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum DurationArg {
    Minutes(f64),
    Text(String),
}

impl DurationArg {
    pub fn seconds(&self) -> Result<u64, String> {
        match self {
            Self::Minutes(minutes) => minutes_to_secs(*minutes),
            Self::Text(text) => parse_duration(text),
        }
    }
}

fn minutes_to_secs(minutes: f64) -> Result<u64, String> {
    if minutes.is_finite() && minutes > 0.0 {
        Ok((minutes * 60.0).round().max(1.0) as u64)
    } else {
        Err(format!("invalid duration: {minutes} minutes"))
    }
}

/// Parses `30s`, `15m`, `1h30m`, `2d`, `1.5h`; a bare number means minutes.
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let text = text.trim().to_ascii_lowercase();
    if let Ok(minutes) = text.parse::<f64>() {
        return minutes_to_secs(minutes);
    }
    let invalid = || format!("invalid duration {text:?}; use forms like 30s, 15m, 1h30m, 2d");
    let mut total = 0.0;
    let mut rest = text.as_str();
    while !rest.trim_start().is_empty() {
        rest = rest.trim_start();
        let number_end = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(rest.len());
        let number: f64 = rest[..number_end].parse().map_err(|_| invalid())?;
        rest = rest[number_end..].trim_start();
        let unit_end = rest
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(rest.len());
        let scale = match &rest[..unit_end] {
            "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
            "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
            "h" | "hr" | "hrs" | "hour" | "hours" => 3_600.0,
            "d" | "day" | "days" => 86_400.0,
            _ => return Err(invalid()),
        };
        total += number * scale;
        rest = &rest[unit_end..];
    }
    if total >= 1.0 {
        Ok(total.round() as u64)
    } else {
        Err(invalid())
    }
}

/// Parses an RFC 3339 datetime, a local `YYYY-MM-DD HH:MM[:SS]` (or with `T`), or a local
/// `HH:MM[:SS]` (today, or tomorrow when that time has passed). Returns epoch milliseconds.
pub fn parse_at(text: &str, now: DateTime<Local>) -> Result<i64, String> {
    let text = text.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(text) {
        return Ok(at.timestamp_millis());
    }
    let formats = [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ];
    if let Some(naive) = formats
        .iter()
        .find_map(|f| NaiveDateTime::parse_from_str(text, f).ok())
    {
        return local_ms(naive, text);
    }
    let time = ["%H:%M:%S", "%H:%M"]
        .iter()
        .find_map(|f| NaiveTime::parse_from_str(text, f).ok())
        .ok_or_else(|| {
            format!("invalid time {text:?}; use RFC 3339, YYYY-MM-DD HH:MM, or HH:MM")
        })?;
    let today: NaiveDate = now.date_naive();
    let at = local_ms(today.and_time(time), text)?;
    if at > now.timestamp_millis() {
        return Ok(at);
    }
    let tomorrow = today
        .succ_opt()
        .ok_or_else(|| format!("invalid time {text:?}"))?;
    local_ms(tomorrow.and_time(time), text)
}

fn local_ms(naive: NaiveDateTime, text: &str) -> Result<i64, String> {
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|at| at.timestamp_millis())
        .ok_or_else(|| format!("{text:?} does not exist in local time"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("30s"), Ok(30));
        assert_eq!(parse_duration("15m"), Ok(900));
        assert_eq!(parse_duration("1h30m"), Ok(5_400));
        assert_eq!(parse_duration("1h 30m"), Ok(5_400));
        assert_eq!(parse_duration("1.5h"), Ok(5_400));
        assert_eq!(parse_duration("2d"), Ok(172_800));
        assert_eq!(parse_duration("10 minutes"), Ok(600));
        assert_eq!(parse_duration("15"), Ok(900));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("5x").is_err());
        assert!(parse_duration("m").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("-5").is_err());
        assert_eq!(DurationArg::Minutes(2.0).seconds(), Ok(120));
        assert_eq!(DurationArg::Text("45s".into()).seconds(), Ok(45));
    }

    #[test]
    fn formats_durations() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(900), "15m");
        assert_eq!(format_duration(5_400), "1h30m");
        assert_eq!(format_duration(90_061), "1d1h1m1s");
    }

    #[test]
    fn parses_datetimes() {
        let now = Local.with_ymd_and_hms(2026, 3, 10, 12, 0, 0).unwrap();
        let at = |y, mo, d, h, mi| {
            Local
                .with_ymd_and_hms(y, mo, d, h, mi, 0)
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(
            parse_at("2026-03-10T08:00:00+01:00", now),
            Ok(chrono::Utc
                .with_ymd_and_hms(2026, 3, 10, 7, 0, 0)
                .unwrap()
                .timestamp_millis())
        );
        assert_eq!(
            parse_at("2026-03-11 09:30", now),
            Ok(at(2026, 3, 11, 9, 30))
        );
        assert_eq!(
            parse_at("2026-03-11T09:30:00", now),
            Ok(at(2026, 3, 11, 9, 30))
        );
        assert_eq!(parse_at("13:15", now), Ok(at(2026, 3, 10, 13, 15)));
        assert_eq!(parse_at("11:00", now), Ok(at(2026, 3, 11, 11, 0)));
        assert!(parse_at("tomorrow", now).is_err());
    }
}
