//! Parses raw terminal input from `ui.key` into keys.

/// One key press. `Text` holds printable input of more than one character (a paste).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Key {
    Enter,
    Escape,
    Tab,
    BackTab,
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Char(char),
    Ctrl(char),
    Alt(char),
    Text(String),
    Unknown(String),
}

impl Key {
    pub fn parse(data: &str) -> Key {
        if let Some(key) = Self::named(data) {
            return key;
        }
        if let Some(key) = Self::csi_u(data) {
            return key;
        }
        if let Some(key) = Self::csi_with_event(data) {
            return key;
        }
        let mut chars = data.chars();
        match (chars.next(), chars.next(), chars.next()) {
            (Some(c), None, _) => Self::single(c),
            (Some('\x1b'), Some(c), None) if !c.is_control() => Key::Alt(c),
            (Some(_), Some(_), _) if !data.chars().any(char::is_control) => {
                Key::Text(data.to_owned())
            }
            _ => Key::Unknown(data.to_owned()),
        }
    }

    fn named(data: &str) -> Option<Key> {
        Some(match data {
            "\r" | "\n" | "\r\n" => Key::Enter,
            "\x1b" => Key::Escape,
            "\t" => Key::Tab,
            "\x1b[Z" => Key::BackTab,
            "\x7f" | "\x08" => Key::Backspace,
            "\x1b[3~" => Key::Delete,
            "\x1b[A" | "\x1bOA" => Key::Up,
            "\x1b[B" | "\x1bOB" => Key::Down,
            "\x1b[C" | "\x1bOC" => Key::Right,
            "\x1b[D" | "\x1bOD" => Key::Left,
            "\x1b[H" | "\x1bOH" | "\x1b[1~" | "\x1b[7~" => Key::Home,
            "\x1b[F" | "\x1bOF" | "\x1b[4~" | "\x1b[8~" => Key::End,
            "\x1b[5~" => Key::PageUp,
            "\x1b[6~" => Key::PageDown,
            _ => return None,
        })
    }

    fn single(c: char) -> Key {
        match c {
            '\x01'..='\x1a' => Key::Ctrl((b'a' + (c as u8 - 1)) as char),
            c if c.is_control() => Key::Unknown(c.to_string()),
            c => Key::Char(c),
        }
    }

    /// Kitty event reports of unmodified cursor and editing keys: `ESC [ 1 ; 1 : 2 A` is a held
    /// Up, `ESC [ 5 ; 1 : 2 ~` a held Page Up. Releases (`: 3`) return `None`.
    fn csi_with_event(data: &str) -> Option<Key> {
        let body = data.strip_prefix("\x1b[")?;
        let last = body.chars().last()?;
        let (params, _) = body.split_at(body.len() - last.len_utf8());
        let (number, mods) = params.split_once(';')?;
        let (mods, event) = mods.split_once(':').unwrap_or((mods, "1"));
        if mods != "1" || event == "3" {
            return None;
        }
        match last {
            'A' | 'B' | 'C' | 'D' | 'H' | 'F' if number == "1" => {
                Self::named(&format!("\x1b[{last}"))
            }
            '~' => Self::named(&format!("\x1b[{number}~")),
            _ => None,
        }
    }

    /// Kitty keyboard protocol: `ESC [ <code> ; <modifiers> u`.
    fn csi_u(data: &str) -> Option<Key> {
        let body = data.strip_prefix("\x1b[")?.strip_suffix('u')?;
        let (code, mods) = body.split_once(';').unwrap_or((body, "1"));
        let code: u32 = code.split(':').next()?.parse().ok()?;
        let mods: u32 = mods.split(':').next()?.parse().ok()?;
        let bits = mods.saturating_sub(1);
        let c = char::from_u32(code)?;
        let key = match code {
            13 => Key::Enter,
            27 => Key::Escape,
            9 if bits & 1 != 0 => Key::BackTab,
            9 => Key::Tab,
            127 => Key::Backspace,
            _ if bits & 4 != 0 => Key::Ctrl(c.to_ascii_lowercase()),
            _ if bits & 2 != 0 => Key::Alt(c),
            _ if bits & 1 != 0 => Key::Char(c.to_ascii_uppercase()),
            _ => Key::Char(c),
        };
        Some(key)
    }

    /// Matches names such as `enter`, `escape`/`esc`, `up`, `ctrl+c`, `alt+x`, `q`, `space`.
    pub fn is(&self, name: &str) -> bool {
        let name = name.to_ascii_lowercase();
        let one_char = |s: &str| {
            let mut chars = s.chars();
            chars.next().filter(|_| chars.next().is_none())
        };
        if let Some(rest) = name.strip_prefix("ctrl+") {
            return one_char(rest).is_some_and(|c| *self == Key::Ctrl(c));
        }
        if let Some(rest) = name.strip_prefix("alt+") {
            return one_char(rest).is_some_and(|c| *self == Key::Alt(c));
        }
        match name.as_str() {
            "enter" | "return" => *self == Key::Enter,
            "escape" | "esc" => *self == Key::Escape,
            "tab" => *self == Key::Tab,
            "shift+tab" | "backtab" => *self == Key::BackTab,
            "backspace" => *self == Key::Backspace,
            "delete" | "del" => *self == Key::Delete,
            "up" => *self == Key::Up,
            "down" => *self == Key::Down,
            "left" => *self == Key::Left,
            "right" => *self == Key::Right,
            "home" => *self == Key::Home,
            "end" => *self == Key::End,
            "pageup" => *self == Key::PageUp,
            "pagedown" => *self == Key::PageDown,
            "space" => *self == Key::Char(' '),
            other => one_char(other).is_some_and(|c| self.char() == Some(c)),
        }
    }

    pub fn char(&self) -> Option<char> {
        match self {
            Key::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        }
    }

    /// True for `escape`, `ctrl+c`, and `q`: the usual keys that close a panel.
    pub fn is_cancel(&self) -> bool {
        self.is("escape") || self.is("ctrl+c") || matches!(self, Key::Char('q'))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_keys() {
        assert_eq!(Key::parse("\r"), Key::Enter);
        assert_eq!(Key::parse("\x1b"), Key::Escape);
        assert_eq!(Key::parse("\x1b[A"), Key::Up);
        assert_eq!(Key::parse("\x1bOB"), Key::Down);
        assert_eq!(Key::parse("\x1b[1;1:2A"), Key::Up);
        assert_eq!(Key::parse("\x1b[1;1:1B"), Key::Down);
        assert_eq!(Key::parse("\x1b[1;1B"), Key::Down);
        assert_eq!(Key::parse("\x1b[5;1:2~"), Key::PageUp);
        assert!(matches!(Key::parse("\x1b[1;1:3A"), Key::Unknown(_)));
        assert!(matches!(Key::parse("\x1b[1;2A"), Key::Unknown(_)));
        assert_eq!(Key::parse("\x03"), Key::Ctrl('c'));
        assert_eq!(Key::parse("q"), Key::Char('q'));
        assert_eq!(Key::parse("\x1bx"), Key::Alt('x'));
        assert_eq!(Key::parse("hello"), Key::Text("hello".into()));
        assert_eq!(Key::parse("\x1b[99;5u"), Key::Ctrl('c'));
        assert_eq!(Key::parse("\x1b[13u"), Key::Enter);
    }

    #[test]
    fn matches_names() {
        assert!(Key::parse("\x03").is("ctrl+c"));
        assert!(Key::parse("\x1b").is("esc"));
        assert!(Key::parse("Q").is("q"));
        assert!(Key::parse(" ").is("space"));
        assert!(!Key::parse("x").is("enter"));
        assert!(Key::parse("q").is_cancel());
    }
}
