use pi_ext_sdk::Key;
use pi_ext_sdk::style;
use serde_json::json;

use super::*;

fn view(value: Value) -> View {
    let mut view = View::new(true, "~/.pi/agent/ext/iterm2.json".into());
    view.set_value(value.as_object().cloned().unwrap());
    view
}

fn press(view: &mut View, keys: &[Key]) -> Action {
    let mut action = Action::Redraw;
    for key in keys {
        action = view.key(key);
    }
    action
}

fn applied(value: Value, edit: Edit) -> Value {
    let mut config = value.as_object().cloned().unwrap();
    edit.apply(&mut config);
    Value::Object(config)
}

#[test]
fn arrows_step_the_theme_and_the_state_colors() {
    let mut v = view(json!({}));
    assert_eq!(
        press(&mut v, &[Key::Right]),
        Action::Save(Edit::Theme(Theme::Light))
    );
    assert_eq!(
        press(&mut v, &[Key::Left]),
        Action::Save(Edit::Theme(Theme::Quiet))
    );
    assert_eq!(
        press(&mut v, &[Key::Down, Key::Right]),
        Action::Save(Edit::Color(ProgramState::Working, Choice::Profile))
    );

    let mut custom = view(json!({"colors": {"done": "#ff8800"}}));
    let action = press(&mut custom, &[Key::Down, Key::Down, Key::Down, Key::Right]);
    assert_eq!(
        action,
        Action::Save(Edit::Color(ProgramState::Done, Choice::Theme))
    );
}

#[test]
fn the_menu_picks_a_theme_and_the_hex_field_saves_a_custom_color() {
    let mut v = view(json!({"theme": "vivid"}));
    press(&mut v, &[Key::Enter]);
    assert_eq!(v.screen, Screen::Menu { choice: 2 });
    assert_eq!(v.preview(), Some(Theme::Vivid.color(ProgramState::Working)));
    let action = press(&mut v, &[Key::Down, Key::Enter]);
    assert_eq!(action, Action::Save(Edit::Theme(Theme::Quiet)));
    assert_eq!(v.screen, Screen::List);

    press(
        &mut v,
        &[Key::Down, Key::Enter, Key::Down, Key::Down, Key::Enter],
    );
    let Screen::Hex { state, text } = &v.screen else {
        panic!("expected the hex field, got {:?}", v.screen);
    };
    assert_eq!((*state, text.as_str()), (ProgramState::Working, "#0969da"));
    let erase = vec![Key::Backspace; 7];
    press(&mut v, &erase);
    press(&mut v, &[Key::Text("FF88".into()), Key::Enter]);
    assert_eq!(v.status.as_ref().map(|s| s.0), Some(Tone::Warn));
    assert_eq!(v.preview(), Some(Theme::Vivid.color(ProgramState::Working)));
    let action = press(&mut v, &[Key::Char('0'), Key::Char('0'), Key::Enter]);
    assert_eq!(
        action,
        Action::Save(Edit::Color(
            ProgramState::Working,
            Choice::Custom([255, 136, 0])
        ))
    );
}

#[test]
fn features_toggle_and_the_preview_returns_to_the_agent_state() {
    let mut v = view(json!({"badge": false}));
    press(&mut v, &[Key::Tab]);
    assert_eq!(v.preview(), None);
    assert_eq!(
        press(&mut v, &[Key::Down, Key::Down, Key::Right]),
        Action::Save(Edit::Feature(Feature::Badge, true))
    );
    assert_eq!(press(&mut v, &[Key::Escape]), Action::Close);
}

#[test]
fn edits_write_only_what_differs_from_the_theme() {
    let start = json!({"enabled": true, "colors": {"done": "profile"}});
    let working = Edit::Color(ProgramState::Working, Choice::Custom([1, 2, 3]));
    assert_eq!(
        applied(start.clone(), working),
        json!({"enabled": true, "colors": {"done": "profile", "working": "#010203"}})
    );
    let done = Edit::Color(ProgramState::Done, Choice::Theme);
    assert_eq!(applied(start.clone(), done), json!({"enabled": true}));
    assert_eq!(
        applied(start.clone(), Edit::ResetColors),
        json!({"enabled": true})
    );
    assert_eq!(
        applied(json!({}), Edit::Theme(Theme::Light)),
        json!({"theme": "light"})
    );
    assert_eq!(
        applied(json!({}), Edit::Feature(Feature::Cwd, false)),
        json!({"cwd": false})
    );
}

#[test]
fn renders_rows_swatches_and_notices_within_the_width() {
    let mut v = view(json!({"colors": {"error": "profile"}}));
    v.iterm2 = false;
    v.project_keys = vec!["theme".into()];
    for width in [40, 100] {
        let lines = v.render(width, None);
        assert!(lines.iter().all(|l| style::visible_width(l) <= width));
        let text: Vec<String> = lines.iter().map(|l| style::strip(l)).collect();
        assert!(
            text.iter()
                .any(|l| l.contains("→ Theme") && l.contains("dark"))
        );
        assert!(
            text.iter()
                .any(|l| l.contains("Working") && l.contains("#1b506a"))
        );
        assert!(
            text.iter()
                .any(|l| l.contains("Error") && l.contains("profile"))
        );
        assert!(text.iter().any(|l| l.contains("not iTerm2")));
        assert!(text.iter().any(|l| l.contains("sets theme")));
    }
    let lines = v.render(100, None);
    assert!(lines.iter().any(|l| l.contains("\x1b[48;2;27;80;106m")));
    assert_eq!(v.render(100, Some(6)).len(), 6);
}

#[test]
fn summary_lists_colors_and_features() {
    let v = view(json!({"theme": "quiet", "colors": {"working": "#ff8800"}, "cwd": false}));
    let summary = v.summary();
    assert!(summary.contains("theme: quiet"));
    assert!(summary.contains("working: #ff8800 (custom)"));
    assert!(summary.contains("done: profile (theme)"));
    assert!(summary.contains("cwd: off"));
}
