//! Display helpers ported from upstream `goal-core.ts`, plus an ISO 8601 parser for waits.

use crate::state::{Goal, GoalStatus, StopReason};

pub fn truncate_text(value: &str, max: usize) -> String {
    let one_line = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let cut: String = one_line.chars().take(max.saturating_sub(3)).collect();
    format!("{cut}...")
}

/// The `Objective:` line of a structured objective, else its first content line.
pub fn objective_title(objective: &str) -> String {
    for line in objective.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("===") {
            continue;
        }
        if let Some(rest) = lower.strip_prefix("objective") {
            if rest.trim_start().starts_with(':') {
                let value = &line[line.find(':').map_or(0, |i| i + 1)..];
                return truncate_text(value, 120);
            }
        }
        return truncate_text(line, 120);
    }
    truncate_text(objective, 120)
}

pub fn format_tokens(value: u64) -> String {
    let compact = |div: f64, unit: &str, big: u64| {
        let n = value as f64 / div;
        let text = if value >= big {
            format!("{n:.0}")
        } else {
            format!("{n:.1}")
        };
        format!("{}{unit}", text.trim_end_matches(".0"))
    };
    match value {
        v if v >= 1_000_000_000 => compact(1e9, "B", 10_000_000_000),
        v if v >= 1_000_000 => compact(1e6, "M", 10_000_000),
        v if v >= 1_000 => compact(1e3, "K", 10_000),
        v => v.to_string(),
    }
}

pub fn format_duration(seconds: u64) -> String {
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}h{m:02}m{s:02}s")
    } else if m > 0 {
        format!("{m}m{s:02}s")
    } else {
        format!("{s}s")
    }
}

pub fn status_label(goal: &Goal) -> String {
    let prefix = if goal.sisyphus { "sisyphus " } else { "" };
    let label = match goal.status {
        GoalStatus::Active if goal.auto_continue => "running",
        GoalStatus::Active => "active",
        GoalStatus::Paused if goal.stop_reason == Some(StopReason::Agent) => "paused (agent)",
        GoalStatus::Paused => "paused",
        GoalStatus::Blocked => "blocked",
        GoalStatus::BudgetLimited => "budget limited",
        GoalStatus::Auditing => "auditing",
        GoalStatus::Complete => "complete",
    };
    format!("{prefix}{label}")
}

pub fn usage_bits(goal: &Goal) -> String {
    let mut bits = Vec::new();
    if goal.usage.active_seconds > 0 {
        bits.push(format_duration(goal.usage.active_seconds));
    }
    if goal.usage.tokens_used > 0 {
        bits.push(format_tokens(goal.usage.tokens_used));
    }
    if bits.is_empty() {
        String::new()
    } else {
        format!(" [{}]", bits.join(" "))
    }
}

pub fn footer_status(goal: &Goal) -> String {
    let prefix = if goal.sisyphus { "goal✊" } else { "goal" };
    format!(
        "{prefix}: {}{} - {}",
        status_label(goal),
        usage_bits(goal),
        truncate_text(&objective_title(&goal.objective), 60)
    )
}

/// Unix milliseconds of `YYYY-MM-DDTHH:MM[:SS[.fff]](Z|±HH:MM)`.
pub fn parse_iso_ms(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once(['T', ' '])?;
    let mut parts = date.splitn(3, '-').map(|p| p.parse::<i64>().ok());
    let (y, mo, d) = (parts.next()??, parts.next()??, parts.next()??);
    let tz_at = rest.rfind(['Z', 'z', '+', '-']).filter(|i| *i >= 5);
    let (clock, tz) = tz_at.map_or((rest, ""), |i| rest.split_at(i));
    let mut fields = clock.splitn(3, ':');
    let h: i64 = fields.next()?.parse().ok()?;
    let mi: i64 = fields.next()?.parse().ok()?;
    let secs: f64 = fields.next().map_or(Some(0.0), |s| s.parse().ok())?;
    let offset_min = match tz {
        "" | "Z" | "z" => 0,
        tz => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            let (oh, om) = tz[1..].split_once(':').unwrap_or((&tz[1..], "0"));
            sign * (oh.parse::<i64>().ok()? * 60 + om.parse::<i64>().ok()?)
        }
    };
    let valid = (1..=12).contains(&mo) && (1..=31).contains(&d) && h < 24 && mi < 60;
    if !valid || !(0.0..61.0).contains(&secs) {
        return None;
    }
    let days = days_from_civil(y, mo, d);
    let ms = ((days * 86_400 + h * 3600 + mi * 60 - offset_min * 60) * 1000) as f64 + secs * 1000.0;
    (ms >= 0.0).then_some(ms as u64)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The text after a `Verification contract:` header line of the objective.
pub fn extract_verification_contract(objective: &str) -> Option<String> {
    let mut lines = objective.lines().map(str::trim);
    let first = lines.find_map(|line| {
        let lower = line.to_ascii_lowercase();
        lower
            .strip_prefix("verification contract")
            .and_then(|rest| rest.trim_start().strip_prefix(':'))
            .map(|_| {
                line[line.find(':').map_or(0, |i| i + 1)..]
                    .trim()
                    .to_owned()
            })
    })?;
    let mut text = first;
    for line in lines {
        if line.is_empty() || is_section_header(line) {
            break;
        }
        text.push('\n');
        text.push_str(line);
    }
    Some(text.trim().to_owned()).filter(|text| !text.is_empty())
}

fn is_section_header(line: &str) -> bool {
    let Some((head, _)) = line.split_once(':') else {
        return false;
    };
    const HEADERS: &[&str] = &[
        "objective",
        "success criteria",
        "boundaries",
        "constraints",
        "ordered steps",
        "steps",
        "if blocked",
        "if blocked / unclear / failing",
        "sisyphus reminder",
    ];
    HEADERS.contains(&head.trim().to_ascii_lowercase().as_str())
}
