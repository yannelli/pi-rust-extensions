//! Five-field cron expressions (`minute hour day-of-month month day-of-week`) in local time.

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Timelike};

/// Years searched for the next match before giving up (for example on `0 0 30 2 *`).
const SEARCH_YEARS: i32 = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cron {
    minutes: u64,
    hours: u64,
    days: u64,
    months: u64,
    weekdays: u64,
    any_day: bool,
    any_weekday: bool,
}

impl Cron {
    pub fn parse(expr: &str) -> Result<Self, String> {
        let fields: Vec<&str> = expr.split_whitespace().collect();
        let [minute, hour, day, month, weekday] = fields[..] else {
            return Err(format!(
                "cron {expr:?} needs 5 fields: minute hour day-of-month month day-of-week"
            ));
        };
        let mut weekdays = field(weekday, 0, 7)?;
        if weekdays & (1 << 7) != 0 {
            weekdays = (weekdays | 1) & !(1 << 7);
        }
        Ok(Self {
            minutes: field(minute, 0, 59)?,
            hours: field(hour, 0, 23)?,
            days: field(day, 1, 31)?,
            months: field(month, 1, 12)?,
            weekdays,
            any_day: day.starts_with('*'),
            any_weekday: weekday.starts_with('*'),
        })
    }

    /// When day-of-month and day-of-week are both restricted, either one matching is enough.
    fn day_matches(&self, date: NaiveDate) -> bool {
        let day = self.days & (1 << date.day()) != 0;
        let weekday = self.weekdays & (1 << date.weekday().num_days_from_sunday()) != 0;
        if self.any_day || self.any_weekday {
            day && weekday
        } else {
            day || weekday
        }
    }

    /// The first matching minute at or after `from` (seconds ignored).
    pub fn next_match(&self, from: NaiveDateTime) -> Option<NaiveDateTime> {
        let mut at = from.date().and_hms_opt(from.hour(), from.minute(), 0)?;
        let last_year = from.year() + SEARCH_YEARS;
        while at.year() <= last_year {
            let date = at.date();
            if self.months & (1 << date.month()) == 0 {
                let (year, month) = if date.month() == 12 {
                    (date.year() + 1, 1)
                } else {
                    (date.year(), date.month() + 1)
                };
                at = NaiveDate::from_ymd_opt(year, month, 1)?.and_hms_opt(0, 0, 0)?;
            } else if !self.day_matches(date) {
                at = date.succ_opt()?.and_hms_opt(0, 0, 0)?;
            } else if self.hours & (1 << at.hour()) == 0 {
                at = date.and_hms_opt(at.hour(), 0, 0)? + Duration::hours(1);
            } else if self.minutes & (1 << at.minute()) == 0 {
                at += Duration::minutes(1);
            } else {
                return Some(at);
            }
        }
        None
    }

    /// The first matching local time strictly after `after`. Skips minutes that do not exist
    /// in local time (DST gaps) and takes the earlier of two ambiguous ones.
    pub fn next_after(&self, after: DateTime<Local>) -> Option<DateTime<Local>> {
        let mut from = after.naive_local() + Duration::minutes(1);
        loop {
            let naive = self.next_match(from)?;
            match Local.from_local_datetime(&naive).earliest() {
                Some(at) if at > after => return Some(at),
                _ => from = naive + Duration::minutes(1),
            }
        }
    }
}

/// A bit mask of the values of one field.
fn field(text: &str, min: u32, max: u32) -> Result<u64, String> {
    let invalid = || format!("invalid cron field {text:?} (allowed {min}-{max})");
    let mut mask = 0u64;
    for part in text.split(',') {
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => (range, step.parse::<u32>().map_err(|_| invalid())?),
            None => (part, 1),
        };
        let (start, end) = match range {
            "*" => (min, max),
            _ => match range.split_once('-') {
                Some((a, b)) => (
                    a.parse().map_err(|_| invalid())?,
                    b.parse().map_err(|_| invalid())?,
                ),
                None => {
                    let value = range.parse().map_err(|_| invalid())?;
                    (value, if part.contains('/') { max } else { value })
                }
            },
        };
        if step == 0 || start < min || end > max || start > end {
            return Err(invalid());
        }
        for value in (start..=end).step_by(step as usize) {
            mask |= 1 << value;
        }
    }
    Ok(mask)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, mo, d)
            .unwrap()
            .and_hms_opt(h, mi, 0)
            .unwrap()
    }

    fn next(expr: &str, from: NaiveDateTime) -> Option<NaiveDateTime> {
        Cron::parse(expr).unwrap().next_match(from)
    }

    #[test]
    fn parses_fields() {
        assert_eq!(field("*", 0, 3), Ok(0b1111));
        assert_eq!(field("*/2", 0, 5), Ok(0b10101));
        assert_eq!(field("1-3", 0, 5), Ok(0b1110));
        assert_eq!(field("0-6/3", 0, 7), Ok(0b100_1001));
        assert_eq!(field("1,4", 0, 5), Ok(0b10010));
        assert_eq!(field("2/2", 0, 6), Ok(0b101_0100));
        for bad in ["", "x", "7", "3-1", "*/0", "1-", "-1"] {
            assert!(field(bad, 0, 6).is_err(), "{bad}");
        }
        assert!(Cron::parse("* * * *").is_err());
        assert!(Cron::parse("60 * * * *").is_err());
        assert!(Cron::parse("* * 0 * *").is_err());
        assert_eq!(
            Cron::parse("0 0 * * 7").unwrap().weekdays,
            Cron::parse("0 0 * * 0").unwrap().weekdays
        );
    }

    #[test]
    fn finds_next_match() {
        let from = at(2026, 3, 10, 12, 7); // a Tuesday
        assert_eq!(next("* * * * *", from), Some(from));
        assert_eq!(next("*/15 * * * *", from), Some(at(2026, 3, 10, 12, 15)));
        assert_eq!(next("0 9 * * *", from), Some(at(2026, 3, 11, 9, 0)));
        assert_eq!(next("30 8 1 * *", from), Some(at(2026, 4, 1, 8, 30)));
        assert_eq!(next("0 0 1 1 *", from), Some(at(2027, 1, 1, 0, 0)));
        assert_eq!(
            next("0 9 * * 1-5", at(2026, 3, 13, 10, 0)),
            Some(at(2026, 3, 16, 9, 0))
        );
        assert_eq!(next("0 9 * * 0", from), Some(at(2026, 3, 15, 9, 0)));
        assert_eq!(next("0 9 * * 7", from), Some(at(2026, 3, 15, 9, 0)));
        assert_eq!(next("0 0 29 2 *", from), Some(at(2028, 2, 29, 0, 0)));
        assert_eq!(next("0 0 30 2 *", from), None);
        assert_eq!(next("59 23 31 12 *", from), Some(at(2026, 12, 31, 23, 59)));
    }

    #[test]
    fn day_of_month_or_day_of_week() {
        // Both restricted: the 15th or any Friday.
        let from = at(2026, 3, 10, 12, 0);
        assert_eq!(next("0 0 15 * 5", from), Some(at(2026, 3, 13, 0, 0)));
        assert_eq!(
            next("0 0 15 * 5", at(2026, 3, 14, 0, 0)),
            Some(at(2026, 3, 15, 0, 0))
        );
        // A star in either field means both must match.
        assert_eq!(next("0 0 */1 * 5", from), Some(at(2026, 3, 13, 0, 0)));
        assert_eq!(next("0 0 15 * *", from), Some(at(2026, 3, 15, 0, 0)));
    }

    #[test]
    fn next_after_is_strictly_later() {
        let cron = Cron::parse("* * * * *").unwrap();
        let now = Local::now();
        let next = cron.next_after(now).unwrap();
        assert!(next > now && next - now <= Duration::minutes(2));
        assert_eq!(next.second(), 0);
    }
}
