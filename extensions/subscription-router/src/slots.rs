//! Slot ranking and status text for one family (multi-codex `selectFreshSlot`, `failoverSlots`).

use std::collections::HashSet;

use crate::family::Family;
use crate::fsutil::duration;
use crate::usage::{
    CACHE_TTL_MS, FIVE_HOURS_MS, SEVEN_DAYS_MS, Scope, Status, UsageSnapshot, UsageWindow,
};

#[derive(Clone, Debug, PartialEq)]
pub struct SlotInfo {
    pub family: Family,
    pub slot: u32,
    pub snapshot: Option<UsageSnapshot>,
}

impl SlotInfo {
    pub fn provider(&self) -> String {
        self.family.provider(self.slot)
    }

    pub fn name(&self) -> String {
        format!("{} #{}", self.family.short(), self.slot)
    }
}

fn provider_window(snapshot: &UsageSnapshot, duration: i64) -> Option<&UsageWindow> {
    snapshot.window_of(duration)
}

/// Percent left of the longest provider-wide window.
pub fn remaining(snapshot: &UsageSnapshot) -> Option<f64> {
    let window = provider_window(snapshot, SEVEN_DAYS_MS).or_else(|| {
        let provider = snapshot
            .windows
            .iter()
            .filter(|w| w.scope == Scope::Provider);
        provider.max_by_key(|w| w.duration_ms.unwrap_or(0))
    })?;
    Some((100.0 - window.used_percent).clamp(0.0, 100.0))
}

/// The reset of an exhausted five-hour window that is still ahead.
pub fn short_limited_until(snapshot: &UsageSnapshot, now: i64) -> Option<i64> {
    let window = provider_window(snapshot, FIVE_HOURS_MS)?;
    window
        .reset_at
        .filter(|r| window.used_percent >= 100.0 && *r > now)
}

pub fn is_fresh(snapshot: &UsageSnapshot, now: i64) -> bool {
    snapshot.status == Status::Available && now - snapshot.retrieved_at < CACHE_TTL_MS
}

/// The fresh, unlimited slot with the most quota left; ties prefer `current`, then the lowest slot.
pub fn select_fresh(slots: &[SlotInfo], current: Option<u32>, now: i64) -> Option<u32> {
    let fresh: Vec<(u32, f64)> = slots
        .iter()
        .filter_map(|info| {
            let snapshot = info.snapshot.as_ref().filter(|s| is_fresh(s, now))?;
            if short_limited_until(snapshot, now).is_some() {
                return None;
            }
            Some((info.slot, remaining(snapshot)?))
        })
        .collect();
    let best = fresh
        .iter()
        .map(|(_, r)| *r)
        .fold(f64::NEG_INFINITY, f64::max);
    let tied: Vec<u32> = fresh
        .iter()
        .filter(|(_, r)| *r == best)
        .map(|(s, _)| *s)
        .collect();
    current
        .filter(|c| tied.contains(c))
        .or_else(|| tied.into_iter().min())
}

/// Untried slots that are not five-hour limited, most quota left first; unknown quota last.
pub fn failover_order(slots: &[SlotInfo], tried: &HashSet<u32>, now: i64) -> Vec<u32> {
    let mut order: Vec<(u32, f64)> = slots
        .iter()
        .filter(|info| !tried.contains(&info.slot))
        .filter(|info| {
            info.snapshot
                .as_ref()
                .is_none_or(|s| short_limited_until(s, now).is_none())
        })
        .map(|info| {
            let fresh = info.snapshot.as_ref().filter(|s| is_fresh(s, now));
            (info.slot, fresh.and_then(remaining).unwrap_or(-1.0))
        })
        .collect();
    order.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    order.into_iter().map(|(slot, _)| slot).collect()
}

fn percent(value: f64) -> String {
    let rounded = (value * 10.0).round() / 10.0;
    if rounded.fract() == 0.0 {
        format!("{rounded:.0}")
    } else {
        format!("{rounded:.1}")
    }
}

/// The window whose reset matters most: the five-hour one when it is fuller, else the weekly one.
fn shown_reset(snapshot: &UsageSnapshot, now: i64) -> Option<(&'static str, i64)> {
    let short =
        provider_window(snapshot, FIVE_HOURS_MS).filter(|w| w.reset_at.is_some_and(|r| r > now));
    let long =
        provider_window(snapshot, SEVEN_DAYS_MS).filter(|w| w.reset_at.is_some_and(|r| r > now));
    match (short, long) {
        (Some(s), Some(l)) if s.used_percent > l.used_percent => Some(("5h", s.reset_at?)),
        (_, Some(l)) => Some(("7d", l.reset_at?)),
        (Some(s), None) => Some(("5h", s.reset_at?)),
        _ => None,
    }
}

/// Footer text: `Codex #2 · 63% · 7d 2d 4h`, `· 5h limit · 1h 2m`, `· stale`, `· unavailable`.
pub fn status_text(info: &SlotInfo, now: i64) -> String {
    let prefix = info.name();
    let Some(snapshot) = info.snapshot.as_ref().filter(|s| s.measured()) else {
        return format!("{prefix} · unavailable");
    };
    if let Some(until) = short_limited_until(snapshot, now) {
        return format!("{prefix} · 5h limit · {}", duration(until - now));
    }
    if !is_fresh(snapshot, now) {
        return format!("{prefix} · stale");
    }
    let left = remaining(snapshot).map_or("?".into(), percent);
    match shown_reset(snapshot, now) {
        Some((label, reset)) => format!("{prefix} · {left}% · {label} {}", duration(reset - now)),
        None => format!("{prefix} · {left}%"),
    }
}

/// One line for `/codex-status` and the account list.
pub fn status_line(info: &SlotInfo, now: i64) -> String {
    let name = info.family.slot_name(info.slot);
    let Some(snapshot) = info.snapshot.as_ref().filter(|s| s.measured()) else {
        return format!("{name}: unavailable");
    };
    let tier = snapshot
        .tier
        .as_ref()
        .map_or(String::new(), |t| format!(" ({t})"));
    let state = if is_fresh(snapshot, now) {
        "measured"
    } else {
        "stale"
    };
    if let Some(until) = short_limited_until(snapshot, now) {
        return format!(
            "{name}{tier}: five-hour limit reached, resets in {} ({state})",
            duration(until - now)
        );
    }
    let left = remaining(snapshot).map_or("?".into(), percent);
    let reset = shown_reset(snapshot, now).map_or(String::new(), |(label, reset)| {
        let window = if label == "5h" {
            "five-hour"
        } else {
            "seven-day"
        };
        format!("; {window} reset in {}", duration(reset - now))
    });
    format!("{name}{tier}: {left}% of the seven-day quota remaining{reset} ({state})")
}
