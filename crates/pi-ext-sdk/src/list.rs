//! List row overflow and the selected row's horizontal scroll clock.

use std::time::{Duration, Instant};

use crate::{Host, style};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ListOverflow {
    #[default]
    Truncate,
    Wrap,
    Scroll,
}

impl ListOverflow {
    pub fn from_setting(value: Option<&str>) -> Self {
        match value {
            Some("wrap") => Self::Wrap,
            Some("scroll") => Self::Scroll,
            _ => Self::Truncate,
        }
    }

    /// Reads the top-level `listOverflow` setting; reduced motion disables scrolling.
    pub async fn read(host: &Host) -> Self {
        let setting = host.settings::<String>("listOverflow").await.ok().flatten();
        Self::from_setting(setting.as_deref())
            .with_reduced_motion(std::env::var("PI_REDUCED_MOTION").is_ok_and(|value| value == "1"))
    }

    pub fn with_reduced_motion(self, reduced: bool) -> Self {
        if reduced && self == Self::Scroll {
            Self::Truncate
        } else {
            self
        }
    }

    /// `prefix` stays fixed; wrapped continuation lines indent under `text`.
    pub fn row(
        self,
        prefix: &str,
        text: &str,
        width: usize,
        selected: bool,
        offset: usize,
    ) -> Vec<String> {
        let indent = style::visible_width(prefix);
        let available = width.saturating_sub(indent);
        if available == 0 || self == Self::Truncate || (self == Self::Scroll && !selected) {
            return vec![style::truncate(&format!("{prefix}{text}"), width)];
        }
        match self {
            Self::Wrap => wrap_row(prefix, text, available),
            Self::Scroll => {
                let max_offset = style::visible_width(text).saturating_sub(available);
                let offset = offset.min(max_offset);
                let text = if offset == 0 {
                    style::truncate(text, available)
                } else {
                    columns(text, offset, available)
                };
                vec![format!("{prefix}{text}")]
            }
            Self::Truncate => unreachable!(),
        }
    }
}

fn wrap_row(prefix: &str, text: &str, width: usize) -> Vec<String> {
    let indent = " ".repeat(style::visible_width(prefix));
    let mut lines = Vec::new();
    let mut line = prefix.to_owned();
    let mut active = String::new();
    let mut used = 0;
    for (token, escape) in style::tokens(text) {
        if escape {
            active.push_str(token);
            line.push_str(token);
            continue;
        }
        let columns = style::visible_width(token);
        if used + columns > width && used > 0 {
            if !active.is_empty() {
                line.push_str("\x1b[0m");
            }
            lines.push(line);
            line = format!("{indent}{active}");
            used = 0;
        }
        if columns > width {
            line.push_str(&" ".repeat(width));
            used = width;
        } else {
            line.push_str(token);
            used += columns;
        }
    }
    if !active.is_empty() {
        line.push_str("\x1b[0m");
    }
    lines.push(line);
    lines
}

fn columns(text: &str, offset: usize, width: usize) -> String {
    let mut result = String::new();
    let mut column = 0;
    let end = offset.saturating_add(width);
    let mut styled = false;
    for (token, escape) in style::tokens(text) {
        if escape {
            result.push_str(token);
            styled = true;
            continue;
        }
        let size = style::visible_width(token);
        let next = column + size;
        if next > offset && column < end {
            if column < offset || next > end {
                result.push_str(&" ".repeat(next.min(end) - column.max(offset)));
            } else {
                result.push_str(token);
            }
        } else if size == 0 && column > offset && column <= end {
            result.push_str(token);
        }
        column = next;
        if column > end {
            break;
        }
    }
    if styled {
        result.push_str("\x1b[0m");
    }
    result
}

/// Call `tick` on a panel timer and redraw when it returns `true`.
#[derive(Debug, Clone)]
pub struct ListScroll {
    pub offset: usize,
    next_step: Instant,
}

impl ListScroll {
    pub const TICK: Duration = Duration::from_millis(100);
    const PAUSE: Duration = Duration::from_secs(1);

    pub fn new(now: Instant) -> Self {
        Self {
            offset: 0,
            next_step: now + Self::PAUSE,
        }
    }

    /// Reset when the selection, row text, or available width changes.
    pub fn reset(&mut self, now: Instant) {
        *self = Self::new(now);
    }

    pub fn tick(&mut self, now: Instant, max_offset: usize) -> bool {
        if max_offset == 0 || now < self.next_step {
            return false;
        }
        if self.offset >= max_offset {
            self.reset(now);
        } else {
            self.offset += 1;
            self.next_step = now + Self::TICK;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: Vec<String>) -> Vec<String> {
        lines.iter().map(|line| style::strip(line)).collect()
    }

    #[test]
    fn setting_defaults_and_reduced_motion() {
        for value in [None, Some("invalid"), Some("truncate")] {
            assert_eq!(ListOverflow::from_setting(value), ListOverflow::Truncate);
        }
        assert_eq!(ListOverflow::from_setting(Some("wrap")), ListOverflow::Wrap);
        assert_eq!(
            ListOverflow::from_setting(Some("scroll")),
            ListOverflow::Scroll
        );
        assert_eq!(
            ListOverflow::Scroll.with_reduced_motion(true),
            ListOverflow::Truncate
        );
        assert_eq!(
            ListOverflow::Wrap.with_reduced_motion(true),
            ListOverflow::Wrap
        );
    }

    #[test]
    fn rows_truncate_wrap_and_scroll_with_a_fixed_prefix() {
        let text = style::dim("abcdefghij");
        assert_eq!(
            plain(ListOverflow::Truncate.row("→ ", &text, 6, true, 0)),
            ["→ abc…"]
        );
        assert_eq!(
            plain(ListOverflow::Wrap.row("→ ", &text, 6, true, 0)),
            ["→ abcd", "  efgh", "  ij"]
        );
        assert_eq!(
            plain(ListOverflow::Scroll.row("→ ", &text, 6, true, 2)),
            ["→ cdef"]
        );
        assert_eq!(
            plain(ListOverflow::Scroll.row("  ", &text, 6, false, 2)),
            ["  abc…"]
        );
        assert_eq!(
            plain(ListOverflow::Scroll.row("→ ", &text, 6, true, 99)),
            ["→ ghij"]
        );
    }

    #[test]
    fn rows_preserve_wide_characters_and_ansi_styles() {
        let text = style::bold("a日本b");
        let wrapped = ListOverflow::Wrap.row("→ ", &text, 6, true, 0);
        assert_eq!(plain(wrapped.clone()), ["→ a日", "  本b"]);
        assert!(wrapped.iter().all(|line| style::visible_width(line) <= 6));
        assert!(wrapped[1].contains("\x1b[1m"));
        assert_eq!(
            plain(ListOverflow::Scroll.row("", &text, 3, true, 2)),
            [" 本"]
        );
        assert_eq!(ListOverflow::Wrap.row("→ ", &text, 0, true, 0), [""]);
    }

    #[test]
    fn scrolling_pauses_steps_and_returns_to_start() {
        let start = Instant::now();
        let mut scroll = ListScroll::new(start);
        assert!(!scroll.tick(start + Duration::from_millis(999), 2));
        assert!(scroll.tick(start + Duration::from_secs(1), 2));
        assert_eq!(scroll.offset, 1);
        assert!(scroll.tick(start + Duration::from_millis(1100), 2));
        assert_eq!(scroll.offset, 2);
        assert!(scroll.tick(start + Duration::from_millis(1200), 2));
        assert_eq!(scroll.offset, 0);
        assert!(!scroll.tick(start + Duration::from_secs(2), 2));
        scroll.reset(start);
        assert!(!scroll.tick(start + Duration::from_secs(2), 0));
    }
}
