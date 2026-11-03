//! Chip shapes: how the colored segments on one side of a status line are drawn and joined.

use crate::colors::{RESET, ThemeColor, background, foreground, shade};
use crate::themes::ink_for;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    /// Colored text joined by the separator.
    Plain,
    /// Text on a colored block.
    Block,
    /// A block with round ends (Nerd Font).
    Rounded,
    /// Blocks joined by arrows (Powerline or Nerd Font).
    Powerline,
    /// Blocks joined by slants (Nerd Font).
    Slanted,
    /// `[text]` in color.
    Brackets,
    /// A colored dot before the text.
    Dot,
}

impl Chip {
    pub const NAMES: &str = "plain, block, rounded, powerline, slanted, brackets, or dot";

    /// The separator for this shape when the settings name none.
    pub fn default_separator(self) -> &'static str {
        match self {
            Chip::Plain => " • ",
            Chip::Dot => "  ",
            _ => " ",
        }
    }

    /// A chip name; `pills` is the older name of `block`.
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "plain" => Some(Chip::Plain),
            "block" | "pills" => Some(Chip::Block),
            "rounded" => Some(Chip::Rounded),
            "powerline" => Some(Chip::Powerline),
            "slanted" => Some(Chip::Slanted),
            "brackets" => Some(Chip::Brackets),
            "dot" => Some(Chip::Dot),
            _ => None,
        }
    }
}

/// The text color of a `plain` chip: a pi theme token or a theme color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fg {
    Token(ThemeColor),
    Rgb([u8; 3]),
}

impl Fg {
    /// The color as sRGB; a token uses its stand-in, as a block background does.
    pub fn rgb(self) -> [u8; 3] {
        match self {
            Fg::Token(token) => token.rgb(),
            Fg::Rgb(rgb) => rgb,
        }
    }
}

/// One segment to draw. `bg` is the block color; `fg` is the color for text-only shapes and the dot;
/// `ink`, when set, paints the text of every shape in place of `fg` and of the contrast pick on blocks.
pub struct Piece {
    pub text: String,
    pub fg: Fg,
    pub bg: [u8; 3],
    pub ink: Option<Fg>,
    /// Per-part colors for the cells of a context bar inside `text`.
    pub bar: Option<BarColors>,
}

/// The cells of a context bar: they start at char `start` of the piece text, and each has the
/// index of its part among `parts`, or `None` when empty.
#[derive(Clone, Debug, PartialEq)]
pub struct BarColors {
    pub start: usize,
    pub cells: Vec<Option<usize>>,
    pub parts: usize,
}

/// The piece text for a piece painted in `base` (`None`: the default color). Bar cells get a
/// shade of `base` each, and the text after them returns to `base`.
fn body(piece: &Piece, base: Option<Fg>) -> String {
    let Some(bar) = &piece.bar else {
        return piece.text.clone();
    };
    let rgb = base.unwrap_or(piece.fg).rgb();
    let reopen = match base {
        Some(Fg::Token(token)) => token.open(),
        Some(Fg::Rgb(rgb)) => foreground(rgb),
        None => "\x1b[39m".to_string(),
    };
    let mut chars = piece.text.chars();
    let mut out: String = chars.by_ref().take(bar.start).collect();
    for part in &bar.cells {
        let Some(glyph) = chars.next() else { break };
        if let Some(part) = part {
            out.push_str(&foreground(shade(rgb, *part, bar.parts)));
        }
        out.push(glyph);
    }
    out.push_str(&reopen);
    out.extend(chars);
    out
}

/// Draws `pieces`; `right` mirrors the arrows of `powerline` and `slanted` for a right-aligned group.
pub fn join(chip: Chip, pieces: &[Piece], separator: &str, right: bool) -> String {
    let fg = foreground;
    let bg = background;
    let paint = |color: Fg, text: &str| match color {
        Fg::Token(token) => token.paint(text),
        Fg::Rgb(rgb) => format!("{}{text}{RESET}", fg(rgb)),
    };
    let colored = |piece: &Piece, wrap: &dyn Fn(String) -> String| {
        let base = piece.ink.unwrap_or(piece.fg);
        paint(base, &wrap(body(piece, Some(base))))
    };
    let block = |piece: &Piece| match piece.ink {
        Some(ink) => format!(
            "{}{}",
            bg(piece.bg),
            paint(ink, &format!(" {} ", body(piece, Some(ink))))
        ),
        None => {
            let ink = ink_for(piece.bg);
            let text = body(piece, Some(Fg::Rgb(ink)));
            format!("{}{} {text} ", bg(piece.bg), fg(ink))
        }
    };
    let joined = |parts: Vec<String>| parts.join(&ThemeColor::Dim.paint(separator));
    match chip {
        Chip::Plain => joined(
            pieces
                .iter()
                .map(|piece| colored(piece, &|text| text))
                .collect(),
        ),
        Chip::Brackets => joined(
            pieces
                .iter()
                .map(|piece| colored(piece, &|text| format!("[{text}]")))
                .collect(),
        ),
        Chip::Dot => joined(
            pieces
                .iter()
                .map(|piece| {
                    let text = piece.ink.map_or_else(
                        || body(piece, None),
                        |ink| paint(ink, &body(piece, Some(ink))),
                    );
                    format!("{} {text}", paint(piece.fg, "●"))
                })
                .collect(),
        ),
        Chip::Block => joined(
            pieces
                .iter()
                .map(|piece| format!("{}{RESET}", block(piece)))
                .collect(),
        ),
        Chip::Rounded => joined(
            pieces
                .iter()
                .map(|piece| {
                    format!(
                        "{}\u{e0b6}{}{RESET}{}\u{e0b4}{RESET}",
                        fg(piece.bg),
                        block(piece),
                        fg(piece.bg)
                    )
                })
                .collect(),
        ),
        Chip::Powerline | Chip::Slanted => {
            let (left_cap, right_cap) = if chip == Chip::Powerline {
                ('\u{e0b0}', '\u{e0b2}')
            } else {
                ('\u{e0bc}', '\u{e0ba}')
            };
            let mut out = String::new();
            for (index, piece) in pieces.iter().enumerate() {
                let previous = index.checked_sub(1).map(|index| pieces[index].bg);
                if right {
                    let behind = previous.map_or_else(|| "\x1b[49m".to_string(), bg);
                    out.push_str(&format!(
                        "{}{behind}{right_cap}{}",
                        fg(piece.bg),
                        block(piece)
                    ));
                } else {
                    if let Some(previous) = previous {
                        out.push_str(&format!("{}{}{left_cap}", fg(previous), bg(piece.bg)));
                    }
                    out.push_str(&block(piece));
                }
            }
            match pieces.last() {
                Some(last) if !right => {
                    out.push_str(&format!("{RESET}{}{left_cap}{RESET}", fg(last.bg)))
                }
                Some(_) => out.push_str(RESET),
                None => {}
            }
            out
        }
    }
}
