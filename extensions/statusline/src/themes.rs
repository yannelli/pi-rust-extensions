//! Status line color themes. Each theme gives one color per role; chip shapes use it as the
//! background (with dark or light ink picked by contrast) and `plain` uses it as the text color.

/// What a segment shows, which picks its color in a theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Path,
    Git,
    Model,
    Thinking,
    Session,
    Tokens,
    Cache,
    Cost,
    ContextOk,
    ContextWarn,
    ContextHigh,
    Time,
    Meta,
    State,
}

const ROLES: usize = 14;

/// `(name, colors by role in [`Role`] order)`.
const THEMES: [(&str, [&str; ROLES]); 23] = [
    (
        "pi-rust",
        [
            "#C9A27E", "#E6984F", "#DE5E28", "#A33B1A", "#B87333", "#C9A27E", "#6FB3B8", "#8FBF7F",
            "#8FBF7F", "#E6984F", "#E5534B", "#6FB3B8", "#7A6A60", "#DE5E28",
        ],
    ),
    (
        "pi",
        [
            "#4D9ABF", "#F1BE58", "#F09082", "#E4A08E", "#A3C4D9", "#A3C4D9", "#8FC1B5", "#9CC58F",
            "#9CC58F", "#F1BE58", "#E5534B", "#4D9ABF", "#8A8F98", "#F09082",
        ],
    ),
    (
        "claude",
        [
            "#E8E6DC", "#CC9B7A", "#D97757", "#C15F3C", "#CFC9B8", "#A3A08F", "#B9B49F", "#8A9A5B",
            "#8A9A5B", "#D9A757", "#BF4D43", "#B0AEA5", "#9C9A92", "#D97757",
        ],
    ),
    (
        "codex",
        [
            "#E5E5E5", "#A3A3A3", "#10A37F", "#0E8A6B", "#D4D4D4", "#737373", "#6B7280", "#10A37F",
            "#10A37F", "#E6A23C", "#EF4444", "#A3A3A3", "#525252", "#10A37F",
        ],
    ),
    (
        "cursor",
        [
            "#374151", "#4B5563", "#F5F5F5", "#9CA3AF", "#6B7280", "#6B7280", "#4B5563", "#3B82F6",
            "#22C55E", "#EAB308", "#EF4444", "#374151", "#1F2937", "#3B82F6",
        ],
    ),
    (
        "dracula",
        [
            "#6272A4", "#F1FA8C", "#BD93F9", "#FF79C6", "#8BE9FD", "#8BE9FD", "#8BE9FD", "#50FA7B",
            "#50FA7B", "#FFB86C", "#FF5555", "#6272A4", "#44475A", "#FF79C6",
        ],
    ),
    (
        "ghostty",
        [
            "#81A2BE", "#F0C674", "#B294BB", "#C9A5CE", "#C5C8C6", "#C5C8C6", "#8ABEB7", "#B5BD68",
            "#B5BD68", "#F0C674", "#E58C8C", "#81A2BE", "#969896", "#B294BB",
        ],
    ),
    (
        "darcula-islands",
        [
            "#82B8F8", "#E2C077", "#56A8F5", "#CF8BC5", "#BCBEC4", "#BCBEC4", "#6BC1CA", "#8FBB8B",
            "#8FBB8B", "#E2C077", "#F78C90", "#A1A5AE", "#868A91", "#56A8F5",
        ],
    ),
    (
        "nord",
        [
            "#5E81AC", "#EBCB8B", "#88C0D0", "#81A1C1", "#B48EAD", "#8FBCBB", "#8FBCBB", "#A3BE8C",
            "#A3BE8C", "#EBCB8B", "#BF616A", "#4C566A", "#434C5E", "#88C0D0",
        ],
    ),
    (
        "catppuccin",
        [
            "#89B4FA", "#F9E2AF", "#CBA6F7", "#F5C2E7", "#94E2D5", "#89DCEB", "#94E2D5", "#A6E3A1",
            "#A6E3A1", "#FAB387", "#F38BA8", "#B4BEFE", "#585B70", "#F5C2E7",
        ],
    ),
    (
        "gruvbox",
        [
            "#458588", "#D79921", "#D65D0E", "#B16286", "#689D6A", "#83A598", "#8EC07C", "#98971A",
            "#98971A", "#D79921", "#CC241D", "#504945", "#3C3836", "#FE8019",
        ],
    ),
    (
        "tokyo-night",
        [
            "#7AA2F7", "#E0AF68", "#BB9AF7", "#F7768E", "#7DCFFF", "#2AC3DE", "#73DACA", "#9ECE6A",
            "#9ECE6A", "#E0AF68", "#F7768E", "#565F89", "#414868", "#BB9AF7",
        ],
    ),
    (
        "solarized",
        [
            "#268BD2", "#B58900", "#CB4B16", "#D33682", "#6C71C4", "#2AA198", "#2AA198", "#859900",
            "#859900", "#B58900", "#DC322F", "#586E75", "#073642", "#6C71C4",
        ],
    ),
    (
        "rose-pine",
        [
            "#31748F", "#F6C177", "#EBBCBA", "#C4A7E7", "#9CCFD8", "#9CCFD8", "#9CCFD8", "#9CCFD8",
            "#9CCFD8", "#F6C177", "#EB6F92", "#6E6A86", "#26233A", "#C4A7E7",
        ],
    ),
    (
        "monokai",
        [
            "#66D9EF", "#E6DB74", "#F92672", "#AE81FF", "#A6E22E", "#66D9EF", "#A6E22E", "#A6E22E",
            "#A6E22E", "#FD971F", "#F92672", "#75715E", "#49483E", "#AE81FF",
        ],
    ),
    (
        "one-dark",
        [
            "#61AFEF", "#E5C07B", "#C678DD", "#E06C75", "#56B6C2", "#56B6C2", "#98C379", "#98C379",
            "#98C379", "#D19A66", "#E06C75", "#5C6370", "#3E4451", "#C678DD",
        ],
    ),
    (
        "mono",
        [
            "#D4D4D4", "#A3A3A3", "#F5F5F5", "#BDBDBD", "#8C8C8C", "#8C8C8C", "#8C8C8C", "#D4D4D4",
            "#BDBDBD", "#E0E0E0", "#FFFFFF", "#737373", "#525252", "#F5F5F5",
        ],
    ),
    (
        "ocean",
        [
            "#2F6EA5", "#7FD1E8", "#1F6F8B", "#4FB0C6", "#99C1DE", "#A9D6E5", "#62B6CB", "#56C596",
            "#7ED3B2", "#F2C14E", "#E4572E", "#89A7C2", "#355C7D", "#2A9D8F",
        ],
    ),
    (
        "forest",
        [
            "#6B8F47", "#C9A227", "#2F6B3B", "#8A5A44", "#A3B18A", "#B7C9A0", "#7FB069", "#3F7A33",
            "#9BC53D", "#E0A030", "#C1442E", "#6E5E45", "#4A5D3A", "#3A7D44",
        ],
    ),
    (
        "sunset",
        [
            "#F4A261", "#FFB4A2", "#E76F51", "#8A3FD1", "#F6BD60", "#F7D6E0", "#E5989B", "#FFCB77",
            "#B5E48C", "#FFB627", "#D62828", "#CDB4DB", "#6D597A", "#F28482",
        ],
    ),
    (
        "pastel",
        [
            "#A7C7E7", "#FDFD96", "#C3B1E1", "#F8C8DC", "#B5EAD7", "#C7CEEA", "#E2F0CB", "#FFDAC1",
            "#B5EAD7", "#FFE29A", "#FF9AA2", "#D5D6EA", "#CFCFCF", "#E0BBE4",
        ],
    ),
    (
        "high-contrast",
        [
            "#FFFFFF", "#FFFF00", "#00FFFF", "#FF00FF", "#00FF00", "#FFFFFF", "#00FFFF", "#00FF00",
            "#00FF00", "#FFD700", "#B00020", "#FFFFFF", "#000000", "#FFFF00",
        ],
    ),
    (
        "paper",
        [
            "#E8E2D0", "#F2E6B8", "#D8E2F0", "#EAD7E8", "#E3EAD6", "#ECE6DA", "#DCEBEA", "#E0EFD8",
            "#D4E9C8", "#F5D9A8", "#F3B8B0", "#E6E1D8", "#D9D4C9", "#DCE3F2",
        ],
    ),
];

/// The theme names, for docs and error messages.
pub fn theme_names() -> impl Iterator<Item = &'static str> {
    THEMES.iter().map(|(name, _)| *name)
}

/// A theme's colors; `None` for an unknown name (the `terminal` theme uses pi's theme tokens).
#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    colors: [[u8; 3]; ROLES],
}

impl Palette {
    pub fn named(name: &str) -> Option<Self> {
        let (_, hex) = THEMES.iter().find(|(theme, _)| *theme == name)?;
        let mut colors = [[0; 3]; ROLES];
        for (slot, value) in colors.iter_mut().zip(hex) {
            *slot = parse_hex(value)?;
        }
        Some(Self { colors })
    }

    pub fn rgb(&self, role: Role) -> [u8; 3] {
        self.colors[role as usize]
    }
}

/// `#rrggbb` as bytes.
pub fn parse_hex(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#').filter(|hex| hex.len() == 6)?;
    let channel = |at: usize| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}

/// A badge color: `#rrggbb` or `#rgb`. Theme and segment `color` values keep accepting `#rrggbb` only.
pub fn parse_badge_hex(value: &str) -> Option<[u8; 3]> {
    let hex = value.strip_prefix('#')?;
    if hex.len() != 3 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return parse_hex(value);
    }
    let channel = |at: usize| u8::from_str_radix(&hex[at..=at].repeat(2), 16).ok();
    Some([channel(0)?, channel(1)?, channel(2)?])
}

/// Dark ink on light backgrounds and light ink on dark ones (WCAG relative luminance).
pub fn ink_for([r, g, b]: [u8; 3]) -> [u8; 3] {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    let luminance = 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
    let dark_contrast = (luminance + 0.05) / (linear(27) + 0.05);
    let light_contrast = (linear(245) + 0.05) / (luminance + 0.05);
    if dark_contrast.max(light_contrast) >= 4.5 {
        if dark_contrast >= light_contrast {
            [27, 27, 27]
        } else {
            [245, 245, 245]
        }
    } else if (luminance + 0.05) / 0.05 >= 1.05 / (luminance + 0.05) {
        [0, 0, 0]
    } else {
        [255, 255, 255]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: [u8; 3], b: [u8; 3]) -> f64 {
        let linear = |channel: u8| {
            let value = f64::from(channel) / 255.0;
            if value <= 0.04045 {
                value / 12.92
            } else {
                ((value + 0.055) / 1.055).powf(2.4)
            }
        };
        let lum = |[r, g, b]: [u8; 3]| 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn context_states_stay_distinct_in_every_palette() {
        for name in theme_names() {
            let palette = Palette::named(name).expect("built-in palette");
            let states = [Role::ContextOk, Role::ContextWarn, Role::ContextHigh]
                .map(|role| palette.rgb(role));
            assert!(
                states[0] != states[1] && states[1] != states[2] && states[0] != states[2],
                "{name}"
            );
        }
    }

    #[test]
    fn badge_hex_accepts_short_and_long_forms_only() {
        assert_eq!(parse_badge_hex("#f80"), Some([0xFF, 0x88, 0x00]));
        assert_eq!(parse_badge_hex("#FF8800"), Some([0xFF, 0x88, 0x00]));
        for invalid in ["#ff", "#ffff", "f80", "#gg0", "#ff88001", "#\u{1b}[31m"] {
            assert_eq!(parse_badge_hex(invalid), None, "{invalid}");
        }
        assert_eq!(parse_hex("#f80"), None);
    }

    #[test]
    fn palettes_keep_chip_ink_readable_for_every_role() {
        for name in theme_names() {
            let palette = Palette::named(name).expect("built-in palette");
            for (role, rgb) in palette.colors.into_iter().enumerate() {
                let ratio = contrast(ink_for(rgb), rgb);
                assert!(ratio >= 4.5, "{name} role {role}: {ratio}");
            }
        }
    }
}
