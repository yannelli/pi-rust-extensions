//! Text and role of usage segments, from one provider's `usage.get` entry.

use pi_ext_sdk::{ProviderUsage, UsageWindow};

use crate::themes::Role;

/// `5h 42% · wk 18%`, with `details` `5h 42% (2h13m) · wk 18% (3d)`. `None` when the
/// provider has an error or no windows.
pub fn format_usage(usage: &ProviderUsage, details: bool, now_ms: i64) -> Option<String> {
    if usage.error.is_some() || usage.windows.is_empty() {
        return None;
    }
    let parts: Vec<String> = usage
        .windows
        .iter()
        .map(|window| {
            let text = format!(
                "{} {}",
                short_label(window),
                format_percent(window.used_percent)
            );
            let left = window
                .resets_at
                .as_deref()
                .filter(|_| details)
                .and_then(parse_utc_ms)
                .map(|resets_at| format_duration(resets_at - now_ms));
            match left {
                Some(left) => format!("{text} ({left})"),
                None => text,
            }
        })
        .collect();
    Some(parts.join(" · "))
}

/// By the highest window: above 90% high, above 70% warn, else ok.
pub fn role(usage: &ProviderUsage) -> Role {
    let highest = usage
        .windows
        .iter()
        .map(|window| window.used_percent)
        .fold(0.0, f64::max);
    if highest > 90.0 {
        Role::ContextHigh
    } else if highest > 70.0 {
        Role::ContextWarn
    } else {
        Role::ContextOk
    }
}

pub fn format_percent(percent: f64) -> String {
    format!("{}%", percent.clamp(0.0, 100.0).round() as u64)
}

fn window_name(minutes: u64) -> String {
    match minutes {
        300 => "5h".into(),
        10080 => "wk".into(),
        m if m > 0 && m % 10080 == 0 => format!("{}wk", m / 10080),
        m if m > 0 && m % 1440 == 0 => format!("{}d", m / 1440),
        m if m > 0 && m % 60 == 0 => format!("{}h", m / 60),
        m => format!("{m}m"),
    }
}

/// `Weekly` becomes `wk`, `Primary` and `Secondary` become the window length, and `Models`
/// drops: `Weekly Opus` is `wk Opus`, `Gemini Models: Weekly` is `Gemini wk`.
pub fn short_label(window: &UsageWindow) -> String {
    let length = window.window_minutes.map(window_name);
    let words: Vec<String> = window
        .label
        .split(|c: char| c.is_whitespace() || c == ':')
        .filter_map(|word| match word.to_ascii_lowercase().as_str() {
            "" | "models" | "model" => None,
            "weekly" | "week" => Some("wk".to_string()),
            "primary" | "secondary" if length.is_some() => length.clone(),
            _ => Some(word.to_string()),
        })
        .collect();
    if words.is_empty() {
        return length.unwrap_or_else(|| "?".into());
    }
    words.join(" ")
}

/// `2h13m`, `3d`, `3d4h`, `45m`, `<1m`, or `now` once the time passed.
pub fn format_duration(ms: i64) -> String {
    if ms <= 0 {
        return "now".into();
    }
    let minutes = ms / 60_000;
    match (minutes / 1440, minutes % 1440 / 60, minutes % 60) {
        (0, 0, 0) => "<1m".into(),
        (0, 0, m) => format!("{m}m"),
        (0, h, 0) => format!("{h}h"),
        (0, h, m) => format!("{h}h{m}m"),
        (d, 0, _) => format!("{d}d"),
        (d, h, _) => format!("{d}d{h}h"),
    }
}

/// Unix milliseconds of `YYYY-MM-DDTHH:MM:SS[.fff]Z` (or `+00:00`).
pub fn parse_utc_ms(text: &str) -> Option<i64> {
    let text = text
        .strip_suffix('Z')
        .or_else(|| text.strip_suffix("+00:00"))?;
    let (date, time) = text.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let (time, fraction) = time.split_once('.').unwrap_or((time, ""));
    let mut time = time.splitn(3, ':').map(str::parse::<i64>);
    let (hour, minute, second) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    let millis = format!("{fraction:0<3}").get(..3)?.parse::<i64>().ok()?;
    let seconds = days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second;
    Some(seconds * 1000 + millis)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * ((month + 9) % 12) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(
        label: &str,
        percent: f64,
        minutes: Option<u64>,
        resets: Option<&str>,
    ) -> UsageWindow {
        UsageWindow {
            label: label.into(),
            used_percent: percent,
            resets_at: resets.map(str::to_string),
            window_minutes: minutes,
        }
    }

    fn usage(windows: Vec<UsageWindow>) -> ProviderUsage {
        ProviderUsage {
            provider: "anthropic".into(),
            windows,
            ..ProviderUsage::default()
        }
    }

    #[test]
    fn labels_are_short() {
        let label = |label, minutes| short_label(&window(label, 0.0, minutes, None));
        assert_eq!(label("5h", Some(300)), "5h");
        assert_eq!(label("Weekly", Some(10080)), "wk");
        assert_eq!(label("Weekly Opus", Some(10080)), "wk Opus");
        assert_eq!(label("Primary", Some(300)), "5h");
        assert_eq!(label("Secondary", Some(1440)), "1d");
        assert_eq!(label("Primary", None), "Primary");
        assert_eq!(label("Gemini Models: Weekly", None), "Gemini wk");
        assert_eq!(label("Total", None), "Total");
        assert_eq!(label("", Some(120)), "2h");
        assert_eq!(label("", None), "?");
    }

    #[test]
    fn percents_round_and_clamp() {
        assert_eq!(format_percent(41.6), "42%");
        assert_eq!(format_percent(42.5), "43%");
        assert_eq!(format_percent(0.4), "0%");
        assert_eq!(format_percent(130.0), "100%");
        assert_eq!(format_percent(-3.0), "0%");
    }

    #[test]
    fn durations_are_compact() {
        let minute = 60_000;
        assert_eq!(format_duration(-5), "now");
        assert_eq!(format_duration(30_000), "<1m");
        assert_eq!(format_duration(45 * minute), "45m");
        assert_eq!(format_duration(120 * minute), "2h");
        assert_eq!(format_duration(133 * minute), "2h13m");
        assert_eq!(format_duration(3 * 1440 * minute), "3d");
        assert_eq!(format_duration((3 * 1440 + 4 * 60 + 5) * minute), "3d4h");
    }

    #[test]
    fn reset_times_parse_as_utc() {
        assert_eq!(parse_utc_ms("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_utc_ms("2026-10-30T17:00:00Z"),
            Some(1_793_379_600_000)
        );
        assert_eq!(
            parse_utc_ms("2026-10-30T17:00:00.250+00:00"),
            Some(1_793_379_600_250)
        );
        assert_eq!(parse_utc_ms("2026-10-30T17:00:00+02:00"), None);
        assert_eq!(parse_utc_ms("soon"), None);
    }

    #[test]
    fn usage_formats_with_and_without_reset_times() {
        let now = parse_utc_ms("2026-10-30T14:47:00Z").unwrap();
        let data = usage(vec![
            window("5h", 41.6, Some(300), Some("2026-10-30T17:00:00Z")),
            window("Weekly", 18.0, Some(10080), Some("2026-11-02T14:47:00Z")),
        ]);
        assert_eq!(
            format_usage(&data, false, now).as_deref(),
            Some("5h 42% · wk 18%")
        );
        assert_eq!(
            format_usage(&data, true, now).as_deref(),
            Some("5h 42% (2h13m) · wk 18% (3d)")
        );
        assert_eq!(role(&data), Role::ContextOk);
    }

    #[test]
    fn errors_and_empty_usage_are_hidden() {
        let mut failed = usage(vec![window("5h", 10.0, Some(300), None)]);
        failed.error = Some("not logged in".into());
        assert_eq!(format_usage(&failed, false, 0), None);
        assert_eq!(format_usage(&usage(Vec::new()), true, 0), None);
    }

    #[test]
    fn roles_follow_the_highest_window() {
        let at = |percent| {
            let windows = vec![
                window("5h", 5.0, None, None),
                window("wk", percent, None, None),
            ];
            role(&usage(windows))
        };
        assert_eq!(at(70.0), Role::ContextOk);
        assert_eq!(at(71.0), Role::ContextWarn);
        assert_eq!(at(91.0), Role::ContextHigh);
    }
}
