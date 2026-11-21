use pi_ext_sdk::Key;
use pi_ext_sdk::style;
use serde_json::json;

use super::*;

/// 2 Claude Code rows and 21 pi rows, like the 23-row repro of #135.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = (0..23)
        .map(|i| {
            let (source, name) = if i < 2 {
                (Source::Claude, format!("c{i:02}"))
            } else {
                (Source::Pi, format!("p{i:02}"))
            };
            let handler = json!({ "type": "command", "command": format!("echo {name}") });
            let file = "/h/settings.json".into();
            let def = HookDef::new(source, Scope::Global, file, "Stop", Some(name), handler);
            Row::new(def, true, Trust::Untrusted, None)
        })
        .collect();
    sort_rows(&mut rows);
    rows
}

fn press(view: &mut View, rows: &[Row], key: &str) -> Action {
    view.key(&Key::parse(key), rows)
}

/// Renders and checks the line count and widths; returns plain text lines.
fn render(view: &View, rows: &[Row], width: usize, height: usize) -> Vec<String> {
    let lines = view.render(rows, width, Some(height));
    assert!(
        lines.len() <= height,
        "{width}x{height}: {} lines",
        lines.len()
    );
    for line in &lines {
        assert!(
            style::visible_width(line) <= width,
            "{width}x{height}: {line:?}"
        );
    }
    lines.iter().map(|l| style::strip(l)).collect()
}

/// The text the pointer line of the selected entry holds.
fn selected_text(view: &View, rows: &[Row]) -> String {
    match view.selected_entry(rows).unwrap() {
        Entry::Hook(row) => row.matcher().to_owned(),
        Entry::Add => "Add a hook".to_owned(),
        Entry::Setting(setting) => setting.label().to_owned(),
    }
}

fn assert_selected_visible(view: &View, rows: &[Row], lines: &[String]) {
    let text = selected_text(view, rows);
    let pointer: Vec<&String> = lines
        .iter()
        .filter(|l| l.trim_start().starts_with('→'))
        .collect();
    assert_eq!(pointer.len(), 1, "{lines:#?}");
    let shown = pointer[0].contains(&text[..text.len().min(8)]);
    assert!(shown, "{text}: {lines:#?}");
}

/// Panel heights: 80x12 and 40x16 terminals less 3 host rows, then edge sizes.
const SIZES: [(usize, usize); 6] = [(80, 9), (40, 13), (80, 37), (20, 3), (100, 5), (30, 1)];
const DOWN: &str = "\x1b[B";

#[test]
fn every_list_position_fits_and_shows_the_selection() {
    let rows = rows();
    for (width, height) in SIZES {
        let mut view = View::new(true);
        for _ in 0..=rows.len() {
            let lines = render(&view, &rows, width, height);
            assert_selected_visible(&view, &rows, &lines);
            press(&mut view, &rows, DOWN);
        }
        assert_eq!(view.selected, 0);
        for key in [
            "\x1b[6~", "\x1b[6~", "\x1b[6~", "\x1b[5~", "\x1b[F", "\x1b[H", "\x1b[A",
        ] {
            press(&mut view, &rows, key);
            let lines = render(&view, &rows, width, height);
            assert_selected_visible(&view, &rows, &lines);
        }
    }
}

#[test]
fn small_heights_keep_tabs_counter_and_close_hint() {
    let rows = rows();
    let mut view = View::new(true);
    for _ in 0..8 {
        press(&mut view, &rows, DOWN);
    }
    for (width, height) in [(80, 9), (40, 13)] {
        let text = render(&view, &rows, width, height).join("\n");
        for part in ["Hooks", "[All (23)]", "(9/24)", "esc close"] {
            assert!(
                text.contains(part),
                "{width}x{height} missing {part}:\n{text}"
            );
        }
    }
    let tall = render(&view, &rows, 80, 37).join("\n");
    for part in ["Claude Code (2)", "pi (21)", "tab switch", "esc close"] {
        assert!(tall.contains(part), "missing {part}:\n{tall}");
    }
    assert!(!tall.contains("(9/24)"), "{tall}");
}

#[test]
fn resize_while_open_keeps_the_selection_visible() {
    let rows = rows();
    let mut view = View::new(true);
    for _ in 0..8 {
        press(&mut view, &rows, DOWN);
    }
    for (width, height) in [(80, 37), (80, 9), (40, 13), (80, 9), (80, 37)] {
        let lines = render(&view, &rows, width, height);
        assert_selected_visible(&view, &rows, &lines);
    }
}

#[test]
fn actions_confirm_add_settings_and_status_fit() {
    let rows = rows();
    for (width, height) in SIZES {
        let mut view = View::new(false);
        press(&mut view, &rows, DOWN);
        press(&mut view, &rows, "\r");
        for action in DetailAction::ALL {
            let text = render(&view, &rows, width, height).join("\n");
            if height >= 3 {
                let label = action.label(true);
                let label = &label[..label.len().min(width.saturating_sub(6))];
                assert!(text.contains(&format!("→ {label}")), "{label}:\n{text}");
            }
            press(&mut view, &rows, DOWN);
        }
        press(&mut view, &rows, "\x1b[A");
        press(&mut view, &rows, "\r");
        let text = render(&view, &rows, width, height).join("\n");
        if height >= 3 && width >= 40 {
            assert!(text.contains("any other key"), "{width}x{height}:\n{text}");
        }
        press(&mut view, &rows, "n");
        press(&mut view, &rows, "\x1b[H");
        press(&mut view, &rows, "\r");
        let text = render(&view, &rows, width, height).join("\n");
        if height >= 3 {
            assert!(text.contains("Add a"), "{width}x{height}:\n{text}");
        }
        press(&mut view, &rows, "\x1b");
        press(&mut view, &rows, "\x1b[Z");
        let lines = render(&view, &rows, width, height);
        assert_selected_visible(&view, &rows, &lines);
        press(&mut view, &rows, "\t");
        press(&mut view, &rows, DOWN);
        let status = format!("Disabled Stop c01; saved in /{}", "x/".repeat(60));
        view.status = Some((Tone::Info, status));
        let lines = render(&view, &rows, width, height);
        if height >= 3 {
            assert_selected_visible(&view, &rows, &lines);
        }
        view.status = None;
        press(&mut view, &rows, "p");
        let lines = render(&view, &rows, width, height);
        assert_selected_visible(&view, &rows, &lines);
    }
}

#[test]
fn toggle_and_remove_act_on_the_visible_selection() {
    let rows = rows();
    let mut view = View::new(true);
    for _ in 0..12 {
        press(&mut view, &rows, DOWN);
    }
    let lines = render(&view, &rows, 80, 9);
    assert_selected_visible(&view, &rows, &lines);
    let Some(Entry::Hook(row)) = view.selected_entry(&rows) else {
        panic!("a hook is selected");
    };
    let hash = row.def.hash.clone();
    assert_eq!(
        press(&mut view, &rows, "\x1b[C"),
        Action::Toggle(hash.clone())
    );
    press(&mut view, &rows, "\r");
    press(&mut view, &rows, "\x1b[A");
    press(&mut view, &rows, "\r");
    let text = render(&view, &rows, 80, 9).join("\n");
    assert!(text.contains("Enter removes it"), "{text}");
    assert_eq!(press(&mut view, &rows, "\r"), Action::Remove(hash));
}

#[test]
fn no_height_keeps_the_twelve_row_window() {
    let rows = rows();
    let view = View::new(true);
    let lines: Vec<String> = view.render(&rows, 80, None);
    let shown = lines
        .iter()
        .filter(|l| style::strip(l).contains("echo "))
        .count();
    assert_eq!(shown, 11);
    assert!(lines.iter().any(|l| style::strip(l).contains("(1/24)")));
}
