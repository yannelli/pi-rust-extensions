use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const ENTRY_TYPE: &str = "pi-chrome-tool-change";
const MAX_MINUTES: u64 = 7 * 24 * 60;
pub const NOT_AUTHORIZED: &str =
    "Chrome is not authorized for this session. Ask the user to run /chrome authorize.";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Grant {
    #[default]
    Locked,
    Until(SystemTime),
    Indefinite,
}

impl Grant {
    pub fn for_duration(duration: Option<Duration>, now: SystemTime) -> Self {
        match duration {
            Some(duration) => Self::Until(now + duration),
            None => Self::Indefinite,
        }
    }

    pub fn is_active(&self, now: SystemTime) -> bool {
        match self {
            Self::Locked => false,
            Self::Until(until) => *until > now,
            Self::Indefinite => true,
        }
    }

    pub fn remaining(&self, now: SystemTime) -> Option<Duration> {
        match self {
            Self::Until(until) => Some(until.duration_since(now).unwrap_or_default()),
            _ => None,
        }
    }

    pub fn until_ms(&self) -> Option<u64> {
        match self {
            Self::Until(until) => Some(epoch_ms(*until)),
            _ => None,
        }
    }

    pub fn describe(&self, now: SystemTime) -> String {
        match self {
            Self::Locked => "locked".to_owned(),
            Self::Indefinite => "until revoked".to_owned(),
            Self::Until(_) if !self.is_active(now) => "expired".to_owned(),
            Self::Until(_) => format!(
                "{} left",
                countdown(self.remaining(now).unwrap_or_default())
            ),
        }
    }
}

/// Parses `/chrome authorize` arguments: empty uses the default, `indefinite`, `15m`, `2h`, `45`.
pub fn parse_duration(arg: &str, default_minutes: u64) -> Result<Option<Duration>> {
    let arg = arg.trim().to_ascii_lowercase();
    let minutes = match arg.as_str() {
        "" if default_minutes == 0 => return Ok(None),
        "" => default_minutes.min(MAX_MINUTES),
        "indefinite" | "forever" | "unlimited" => return Ok(None),
        _ => {
            let (number, scale) = match arg.strip_suffix('h') {
                Some(hours) => (hours, 60),
                None => (arg.strip_suffix('m').unwrap_or(&arg), 1),
            };
            match number
                .trim()
                .parse::<u64>()
                .ok()
                .and_then(|n| n.checked_mul(scale))
            {
                Some(n) if (1..=MAX_MINUTES).contains(&n) => n,
                _ => bail!(
                    "Use a duration such as 15m, 30m, 2h, 45, or indefinite (at most 7 days)."
                ),
            }
        }
    };
    Ok(Some(Duration::from_secs(minutes * 60)))
}

pub fn countdown(duration: Duration) -> String {
    let minutes = duration.as_secs().div_ceil(60);
    match minutes {
        0 => "0m".to_owned(),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h {}m", m / 60, m % 60),
    }
}

pub fn epoch_ms(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolChange {
    pub content: String,
    /// `authorized`, `reauthorized`, `expired`, or `revoked`.
    pub action: String,
    pub tools: Vec<String>,
    pub authorized_until: Option<u64>,
    #[serde(default)]
    pub indefinite: bool,
    #[serde(default)]
    pub session_id: Option<String>,
    pub at: u64,
}

impl ToolChange {
    /// Forked or resumed sessions copy entries, so a grant restores only in the session that wrote it.
    pub fn restorable(&self, session_id: Option<&str>, now: SystemTime) -> Grant {
        let same_session = session_id.is_some() && self.session_id.as_deref() == session_id;
        if !same_session || !matches!(self.action.as_str(), "authorized" | "reauthorized") {
            return Grant::Locked;
        }
        let grant = match self.authorized_until {
            Some(ms) => Grant::Until(UNIX_EPOCH + Duration::from_millis(ms)),
            None if self.indefinite => Grant::Indefinite,
            None => Grant::Locked,
        };
        if grant.is_active(now) {
            grant
        } else {
            Grant::Locked
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        let min = |m: u64| Some(Duration::from_secs(m * 60));
        assert_eq!(parse_duration("", 15).unwrap(), min(15));
        assert_eq!(parse_duration("", 0).unwrap(), None);
        assert_eq!(parse_duration("30m", 15).unwrap(), min(30));
        assert_eq!(parse_duration("2h", 15).unwrap(), min(120));
        assert_eq!(parse_duration(" 45 ", 15).unwrap(), min(45));
        assert_eq!(parse_duration("Indefinite", 15).unwrap(), None);
        assert!(parse_duration("0", 15).is_err());
        assert!(parse_duration("soon", 15).is_err());
        assert!(parse_duration("99999999999999999999h", 15).is_err());
        assert!(parse_duration("8000000000000000000h", 15).is_err());
        assert!(parse_duration("10081", 15).is_err());
    }

    #[test]
    fn grant_expires() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let grant = Grant::for_duration(Some(Duration::from_secs(90)), now);
        assert!(grant.is_active(now));
        assert_eq!(grant.describe(now), "2m left");
        let later = now + Duration::from_secs(91);
        assert!(!grant.is_active(later));
        assert_eq!(grant.describe(later), "expired");
        assert!(Grant::Indefinite.is_active(later));
        assert!(!Grant::Locked.is_active(now));
        assert_eq!(countdown(Duration::from_secs(3_900)), "1h 5m");
    }

    #[test]
    fn restores_only_valid_grants() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000);
        let mut change = ToolChange {
            content: String::new(),
            action: "authorized".to_owned(),
            tools: vec![],
            authorized_until: Some(2_000_000),
            indefinite: false,
            session_id: Some("s1".to_owned()),
            at: 0,
        };
        let s1 = Some("s1");
        assert!(matches!(change.restorable(s1, now), Grant::Until(_)));
        assert_eq!(change.restorable(Some("s2"), now), Grant::Locked);
        assert_eq!(change.restorable(None, now), Grant::Locked);
        change.authorized_until = Some(500_000);
        assert_eq!(change.restorable(s1, now), Grant::Locked);
        change.authorized_until = None;
        change.indefinite = true;
        assert_eq!(change.restorable(s1, now), Grant::Indefinite);
        change.action = "revoked".to_owned();
        assert_eq!(change.restorable(s1, now), Grant::Locked);
    }
}
