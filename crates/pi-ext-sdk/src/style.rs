//! ANSI styling and width helpers for widget, footer, and panel lines.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    Gray,
    Ansi256(u8),
    Rgb(u8, u8, u8),
}

impl Color {
    fn sgr(self, background: bool) -> String {
        let base: u8 = if background { 40 } else { 30 };
        let named = |offset: u8| (base + offset).to_string();
        match self {
            Color::Black => named(0),
            Color::Red => named(1),
            Color::Green => named(2),
            Color::Yellow => named(3),
            Color::Blue => named(4),
            Color::Magenta => named(5),
            Color::Cyan => named(6),
            Color::White => named(7),
            Color::Gray => named(60),
            Color::Ansi256(n) => format!("{};5;{n}", base + 8),
            Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
        }
    }
}

fn wrap(code: &str, text: &str, reset: &str) -> String {
    format!("\x1b[{code}m{text}\x1b[{reset}m")
}

pub fn bold(text: &str) -> String {
    wrap("1", text, "22")
}

pub fn dim(text: &str) -> String {
    wrap("2", text, "22")
}

pub fn italic(text: &str) -> String {
    wrap("3", text, "23")
}

pub fn underline(text: &str) -> String {
    wrap("4", text, "24")
}

pub fn inverse(text: &str) -> String {
    wrap("7", text, "27")
}

pub fn fg(color: Color, text: &str) -> String {
    wrap(&color.sgr(false), text, "39")
}

pub fn bg(color: Color, text: &str) -> String {
    wrap(&color.sgr(true), text, "49")
}

/// Splits text into escape sequences (`true`) and plain characters (`false`).
fn tokens(text: &str) -> impl Iterator<Item = (&str, bool)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let first = rest.chars().next()?;
        let len = if first == '\x1b' {
            escape_len(rest)
        } else {
            first.len_utf8()
        };
        let (token, tail) = rest.split_at(len);
        rest = tail;
        Some((token, first == '\x1b'))
    })
}

fn escape_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    match bytes.get(1) {
        Some(b'[') => bytes[2..]
            .iter()
            .position(|b| (0x40..=0x7e).contains(b))
            .map_or(bytes.len(), |i| i + 3),
        Some(b']') => {
            let end = text.find('\x07').map(|i| i + 1);
            let st = text.find("\x1b\\").map(|i| i + 2);
            end.into_iter().chain(st).min().unwrap_or(bytes.len())
        }
        Some(_) => 1 + text[1..].chars().next().map_or(0, char::len_utf8),
        None => 1,
    }
}

/// Removes ANSI escape sequences.
pub fn strip(text: &str) -> String {
    tokens(text)
        .filter(|(_, esc)| !esc)
        .map(|(t, _)| t)
        .collect()
}

/// Terminal columns of a character: 0 for combining marks, 2 for wide CJK and emoji.
pub fn char_width(c: char) -> usize {
    let cp = c as u32;
    let zero = matches!(cp, 0x300..=0x36f | 0x200b..=0x200f | 0x20d0..=0x20ff | 0xfe00..=0xfe0f);
    if zero || c.is_control() {
        return 0;
    }
    let wide = matches!(cp,
        0x1100..=0x115f | 0x2e80..=0x303e | 0x3041..=0xa4cf | 0xac00..=0xd7a3
        | 0xf900..=0xfaff | 0xfe30..=0xfe4f | 0xff00..=0xff60 | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f | 0x1f680..=0x1f6ff | 0x1f900..=0x1f9ff | 0x20000..=0x3fffd);
    if wide { 2 } else { 1 }
}

/// Terminal columns of `text`, ignoring ANSI escape sequences.
pub fn visible_width(text: &str) -> usize {
    tokens(text)
        .filter(|(_, esc)| !esc)
        .flat_map(|(t, _)| t.chars())
        .map(char_width)
        .sum()
}

/// Cuts `text` to `width` columns with a trailing `…`; keeps escape sequences.
pub fn truncate(text: &str, width: usize) -> String {
    truncate_with(text, width, "…")
}

pub fn truncate_with(text: &str, width: usize, ellipsis: &str) -> String {
    if visible_width(text) <= width {
        return text.to_owned();
    }
    let ellipsis_width = visible_width(ellipsis);
    let (budget, ellipsis) = if ellipsis_width > width {
        (width, "")
    } else {
        (width - ellipsis_width, ellipsis)
    };
    let mut out = String::new();
    let mut used = 0;
    let mut styled = false;
    for (token, esc) in tokens(text) {
        if esc {
            styled = true;
            out.push_str(token);
            continue;
        }
        let w = token.chars().map(char_width).sum::<usize>();
        if used + w > budget {
            break;
        }
        used += w;
        out.push_str(token);
    }
    out.push_str(ellipsis);
    if styled {
        out.push_str("\x1b[0m");
    }
    out
}

/// Truncates or pads `text` with spaces to exactly `width` columns.
pub fn fit(text: &str, width: usize) -> String {
    let cut = truncate(text, width);
    let pad = width.saturating_sub(visible_width(&cut));
    format!("{cut}{}", " ".repeat(pad))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths_ignore_escapes() {
        let line = format!("{} {}", bold("todo"), fg(Color::Green, "done"));
        assert_eq!(visible_width(&line), 9);
        assert_eq!(strip(&line), "todo done");
        assert_eq!(visible_width("日本"), 4);
        assert_eq!(visible_width("\x1b]8;;http://x\x07link\x1b]8;;\x07"), 4);
    }

    #[test]
    fn truncates_and_fits() {
        assert_eq!(truncate("hello world", 5), "hell…");
        assert_eq!(truncate("hi", 5), "hi");
        assert_eq!(strip(&truncate(&dim("hello world"), 6)), "hello…");
        assert!(truncate(&dim("hello world"), 6).ends_with("\x1b[0m"));
        assert_eq!(fit("ab", 4), "ab  ");
        assert_eq!(visible_width(&fit("日本語", 5)), 5);
    }
}
