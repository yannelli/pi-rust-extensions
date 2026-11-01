//! Per-badge `foreground` and `background`: filled shapes take the background and pick ink by contrast unless a
//! foreground is set; text-only shapes keep their legacy colors, and an explicit foreground colors every shape's text.

mod support;

use pi_ext_statusline::colors::ThemeColor;
use pi_ext_statusline::layout::{change_layout, layout_values};
use support::*;

const MODEL: &str = "gpt-6.1-sol";

fn fg([r, g, b]: [u8; 3]) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

fn bg([r, g, b]: [u8; 3]) -> String {
    format!("\x1b[48;2;{r};{g};{b}m")
}

fn line(chip: &str, segments: serde_json::Value) -> String {
    first_line(
        json!({"chip": chip, "icons": "none", "segments": segments}),
        100,
    )
}

fn filled(background: [u8; 3], ink: [u8; 3], text: &str) -> String {
    format!("{}{} {text} ", bg(background), fg(ink))
}

#[test]
fn two_copies_of_one_segment_keep_their_own_colors() {
    let drawn = line(
        "block",
        json!([
            {"segment": "model", "background": "#ff0000", "foreground": "#ffffff"},
            {"segment": "model", "background": "#0000ff"}
        ]),
    );
    let blue = [0, 0, 255];
    assert!(
        drawn.contains(&filled([255, 0, 0], [255, 255, 255], MODEL)),
        "{drawn:?}"
    );
    assert!(
        drawn.contains(&filled(blue, ink_for(blue), MODEL)),
        "{drawn:?}"
    );
}

#[test]
fn legacy_color_and_colors_map_keep_their_meaning() {
    let red = [255, 0, 0];
    let plain = line("plain", json!([{"segment": "model", "color": "#ff0000"}]));
    assert!(
        plain.contains(&format!("{}{MODEL}\x1b[0m", fg(red))),
        "{plain:?}"
    );
    let block = line("block", json!([{"segment": "model", "color": "#ff0000"}]));
    assert!(
        block.contains(&filled(red, ink_for(red), MODEL)),
        "{block:?}"
    );
    let mapped = first_line(
        json!({"chip": "plain", "icons": "none", "segments": ["model"], "colors": {"model": "warning"}}),
        100,
    );
    assert!(
        mapped.contains(&ThemeColor::Warning.paint(MODEL)),
        "{mapped:?}"
    );

    let text_only = line(
        "plain",
        json!([{"segment": "model", "color": "#ff0000", "background": "#00ff00"}]),
    );
    assert_eq!(
        text_only, plain,
        "a background does not change text-only shapes"
    );
    let ink = line(
        "plain",
        json!([{"segment": "model", "color": "#ff0000", "foreground": "#00ff00"}]),
    );
    assert!(
        ink.contains(&format!("{}{MODEL}\x1b[0m", fg([0, 255, 0]))),
        "{ink:?}"
    );
    let green = [0, 255, 0];
    let over = line(
        "block",
        json!([{"segment": "model", "color": "#ff0000", "background": "#00ff00"}]),
    );
    assert!(
        over.contains(&filled(green, ink_for(green), MODEL)),
        "{over:?}"
    );
}

#[test]
fn invalid_badge_colors_fall_back_without_reaching_the_terminal() {
    for chip in ["plain", "block", "powerline", "dot"] {
        let base = line(chip, json!([{"segment": "model", "color": "#ff0000"}]));
        for (foreground, background) in [
            ("\u{1b}[31mred", "#12"),
            ("rainbow", "#ggg"),
            ("#12345", "accent;5"),
        ] {
            let drawn = line(
                chip,
                json!([{"segment": "model", "color": "#ff0000", "foreground": foreground, "background": background}]),
            );
            assert_eq!(drawn, base, "{chip} {foreground:?} {background:?}");
            assert!(!drawn.contains("\u{1b}[31mred"), "{drawn:?}");
        }
    }
}

#[test]
fn filled_shapes_pick_ink_by_contrast_unless_a_foreground_is_set() {
    let white = line(
        "block",
        json!([{"segment": "model", "background": "#ffffff"}]),
    );
    assert!(
        white.contains(&filled([255, 255, 255], [27, 27, 27], MODEL)),
        "{white:?}"
    );
    let black = line("block", json!([{"segment": "model", "background": "#000"}]));
    assert!(
        black.contains(&filled([0, 0, 0], [245, 245, 245], MODEL)),
        "{black:?}"
    );
    let token = line(
        "block",
        json!([{"segment": "model", "background": "accent", "foreground": "warning"}]),
    );
    let expected = format!(
        "{}{}",
        bg(ThemeColor::Accent.rgb()),
        ThemeColor::Warning.paint(&format!(" {MODEL} "))
    );
    assert!(
        token.contains(&expected),
        "a background token uses its stand-in, a foreground token its SGR: {token:?}"
    );
}

#[test]
fn every_shape_draws_an_explicit_foreground() {
    let ink = fg([0x12, 0x34, 0x56]);
    let badge = json!([{"segment": "model", "foreground": "#123456"}]);
    for (chip, text) in [
        ("plain", MODEL.to_string()),
        ("brackets", format!("[{MODEL}]")),
        ("dot", MODEL.to_string()),
        ("block", format!(" {MODEL} ")),
        ("rounded", format!(" {MODEL} ")),
        ("powerline", format!(" {MODEL} ")),
        ("slanted", format!(" {MODEL} ")),
    ] {
        let drawn = line(chip, badge.clone());
        assert!(drawn.contains(&format!("{ink}{text}")), "{chip}: {drawn:?}");
    }
    let dot = line("dot", badge);
    assert!(
        !dot.contains(&format!("{ink}●")),
        "the dot keeps its own color: {dot:?}"
    );
    let token = line(
        "plain",
        json!([{"segment": "model", "foreground": "warning"}]),
    );
    assert!(
        token.contains(&ThemeColor::Warning.paint(MODEL)),
        "{token:?}"
    );
    let accent = |text: &str| ThemeColor::Accent.paint(text);
    let accent_badge = json!([{"segment": "model", "foreground": "accent"}]);
    assert!(line("plain", accent_badge.clone()).contains(&accent(MODEL)));
    for chip in ["block", "rounded", "powerline", "slanted"] {
        let drawn = line(chip, accent_badge.clone());
        assert!(
            drawn.contains(&accent(&format!(" {MODEL} "))),
            "{chip} uses the same token: {drawn:?}"
        );
    }
    let default_dot = line("dot", json!(["model"]));
    assert!(
        default_dot.contains(&format!("●\x1b[39m {MODEL}")),
        "{default_dot:?}"
    );
}

#[test]
fn powerline_right_groups_join_explicit_backgrounds_and_reset() {
    let segments = json!([
        "dir",
        "fill",
        {"segment": "model", "background": "#102030"},
        {"segment": "cost", "background": "#405060"}
    ]);
    let (first, second) = ([0x10, 0x20, 0x30], [0x40, 0x50, 0x60]);
    let drawn = line("powerline", segments.clone());
    let opening = format!(
        "{}\x1b[49m\u{e0b2}{}",
        fg(first),
        filled(first, ink_for(first), MODEL)
    );
    let joint = format!("{}{}\u{e0b2}{}", fg(second), bg(first), bg(second));
    assert!(drawn.contains(&opening), "{drawn:?}");
    assert!(drawn.contains(&joint), "{drawn:?}");
    assert!(drawn.ends_with("\x1b[0m"), "{drawn:?}");
    assert_eq!(visible_width(&drawn), 100);

    for width in [40, 24, 12] {
        let narrow = first_line(
            json!({"chip": "powerline", "icons": "none", "segments": segments}),
            width,
        );
        assert!(visible_width(&narrow) <= width, "{width}: {narrow:?}");
        assert!(!strip(&narrow).contains('\u{1b}'), "{width}: {narrow:?}");
    }
}

#[test]
fn layout_values_and_reordering_keep_badge_colors() {
    let badge = json!({"segment": "model", "foreground": "#000", "background": "accent"});
    let statusline = builtin(json!({"segments": [badge.clone(), "dir"]}));
    let mut lines = layout_values(&statusline);
    assert_eq!(lines[0][0], badge);
    change_layout(&mut lines, "0:0", "later");
    assert_eq!(lines[0][1], badge);
    let item = Item::parse(&lines[0][1]).expect("item");
    assert_eq!(
        (item.foreground.as_deref(), item.background.as_deref()),
        (Some("#000"), Some("accent"))
    );
}

#[test]
fn the_new_palettes_color_badges() {
    let names: Vec<&str> = theme_names().collect();
    for theme in [
        "ocean",
        "forest",
        "sunset",
        "pastel",
        "high-contrast",
        "paper",
    ] {
        assert!(names.contains(&theme), "{theme}");
        let palette = Palette::named(theme).expect("palette");
        let model = palette.rgb(Role::Model);
        let drawn = first_line(
            json!({"chip": "block", "theme": theme, "icons": "none", "segments": ["model"]}),
            60,
        );
        assert!(
            drawn.contains(&filled(model, ink_for(model), MODEL)),
            "{theme}: {drawn:?}"
        );
        let warn =
            [Role::ContextOk, Role::ContextWarn, Role::ContextHigh].map(|role| palette.rgb(role));
        assert!(warn[0] != warn[1] && warn[1] != warn[2], "{theme}");
    }
}
