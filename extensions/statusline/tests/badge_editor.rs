use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pi_ext_statusline::badge_colors::custom_hex;
use pi_ext_statusline::editor::StatusLineEditor;
use serde_json::{Value, json};

const ENTER: &str = "\r";
const ESC: &str = "\x1b";
const DOWN: &str = "\x1b[B";
const CTRL_U: &str = "\x15";

fn recording_editor(value: Value) -> (StatusLineEditor, Arc<Mutex<Vec<Value>>>) {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let writes = saved.clone();
    let save = Box::new(move |value: Value| {
        writes.lock().unwrap().push(value);
        Ok(())
    });
    let editor = StatusLineEditor::new(Some(value), PathBuf::from("/nonexistent-home"), save);
    (editor, saved)
}

fn keys(editor: &mut StatusLineEditor, keys: &[&str]) {
    for key in keys {
        editor.handle_input(key);
    }
}

fn open_badges(editor: &mut StatusLineEditor) {
    editor.select_item("badges");
    editor.handle_input(ENTER);
}

fn screen(editor: &StatusLineEditor) -> String {
    editor.render(200, &[]).join("\n")
}

#[test]
fn each_badge_instance_gets_its_own_colors() {
    let value = json!({"segments": [["model", "fill", "model", {"text": "λ"}]], "unknown": 1});
    let (mut editor, saved) = recording_editor(value);
    open_badges(&mut editor);
    let shown = screen(&editor);
    assert!(
        shown.contains("1.1 model") && shown.contains("1.3 model"),
        "{shown}"
    );
    assert!(
        shown.contains("1.4 text \"λ\"") && !shown.contains("1.2"),
        "{shown}"
    );
    keys(&mut editor, &[DOWN, ENTER, ENTER, "warning", ENTER]);
    keys(
        &mut editor,
        &[DOWN, ENTER, "#61afef", ENTER, ESC, DOWN, DOWN, ENTER],
    );
    let saved = saved.lock().unwrap();
    assert_eq!(saved.len(), 1);
    let second = json!({"segment": "model", "foreground": "warning", "background": "#61afef"});
    assert_eq!(
        saved[0]["segments"],
        json!([["model", "fill", second, {"text": "λ"}]])
    );
    assert_eq!(saved[0]["unknown"], 1);
    assert_eq!(saved[0]["type"], "builtin");
}

#[test]
fn custom_hex_is_validated_previewed_and_dropped_on_escape() {
    let original = json!({"type": "builtin", "segments": [["model"]]});
    let (mut editor, saved) = recording_editor(original.clone());
    open_badges(&mut editor);
    keys(&mut editor, &[ENTER, ENTER, DOWN]);
    assert_eq!(
        editor.preview_value()["segments"][0][0]["foreground"],
        "accent"
    );
    keys(&mut editor, &[ESC]);
    assert_eq!(editor.preview_value()["segments"], json!([["model"]]));
    keys(&mut editor, &[ENTER, "custom", ENTER, "#AABBCC"]);
    assert_eq!(
        editor.preview_value()["segments"][0][0]["foreground"],
        "#aabbcc"
    );
    keys(&mut editor, &["Z", ENTER]);
    assert!(screen(&editor).contains("Enter a color as #rrggbb"));
    assert_eq!(editor.preview_value()["segments"], json!([["model"]]));
    keys(&mut editor, &[CTRL_U, "#56B6C2"]);
    assert!(screen(&editor).contains("Hex: #56B6C2"));
    assert_eq!(
        editor.preview_value()["segments"][0][0]["foreground"],
        "#56b6c2"
    );
    keys(&mut editor, &[ESC]);
    assert_eq!(editor.preview_value()["segments"], json!([["model"]]));
    assert!(screen(&editor).contains("Text color"));
    keys(&mut editor, &[ESC, ESC]);
    assert!(saved.lock().unwrap().is_empty());
    assert!(!editor.closed);
    assert_eq!(editor.value, original);
    assert_eq!(editor.preview_value(), original);
    assert_eq!(custom_hex(" #A0b1C2 ").as_deref(), Some("#a0b1c2"));
    for bad in ["#abc", "#+1+2+3", "a0b1c2", "#a0b1c2d"] {
        assert_eq!(custom_hex(bad), None, "{bad}");
    }
}

#[test]
fn reset_removes_only_the_badge_colors() {
    let pill = json!({"segment": "model", "color": "accent", "icon": "M",
        "foreground": "#ffffff", "background": "#000000"});
    let value = json!({"theme": "nord", "unknown": true, "segments": [pill]});
    let (mut editor, saved) = recording_editor(value);
    open_badges(&mut editor);
    assert!(screen(&editor).contains("legacy color accent"));
    keys(&mut editor, &[ENTER, DOWN, DOWN, ENTER, ESC, DOWN, ENTER]);
    let saved = saved.lock().unwrap();
    assert_eq!(
        saved[0]["segments"],
        json!([[{"segment": "model", "color": "accent", "icon": "M"}]])
    );
    assert_eq!(saved[0]["theme"], "nord");
    assert_eq!(saved[0]["unknown"], true);
}

#[test]
fn a_failed_save_keeps_the_settings_and_shows_the_error() {
    let original = json!({"segments": ["model"]});
    let save = Box::new(|_| Err("read only".to_string()));
    let mut editor = StatusLineEditor::new(
        Some(original.clone()),
        PathBuf::from("/nonexistent-home"),
        save,
    );
    open_badges(&mut editor);
    keys(&mut editor, &[ENTER, ENTER, DOWN, ENTER, ESC, DOWN, ENTER]);
    assert_eq!(editor.value, original);
    assert_eq!(editor.error.as_deref(), Some("read only"));
    let shown = screen(&editor);
    assert!(
        shown.contains("read only") && shown.contains("Badge colors"),
        "{shown}"
    );
}

#[test]
fn module_reorder_keeps_badge_colors_and_legacy_attributes() {
    let pill = json!({"segment": "model", "foreground": "#ff0000", "background": "accent",
        "color": "accent", "icon": "M"});
    let (mut editor, saved) = recording_editor(json!({"segments": [[pill.clone(), "dir"]]}));
    editor.select_item("segments");
    keys(
        &mut editor,
        &[ENTER, ENTER, "later", ENTER, DOWN, DOWN, DOWN, DOWN, ENTER],
    );
    let saved = saved.lock().unwrap();
    assert_eq!(saved[0]["segments"], json!([[{"segment": "dir"}, pill]]));
}

#[test]
fn a_project_edits_the_inherited_layout_and_saves_only_its_own_keys() {
    let global = json!({"type": "command", "command": "global.sh", "theme": "nord",
        "segments": [["model", "model"]]});
    let (editor, saved) = recording_editor(json!({"theme": "dracula"}));
    let mut editor = editor.with_badge_defaults(global);
    open_badges(&mut editor);
    keys(&mut editor, &[DOWN, ENTER, ENTER, "error"]);
    let preview = editor.preview_value();
    assert_eq!(preview["theme"], "dracula");
    assert_eq!(preview["type"], "builtin");
    let second = json!({"segment": "model", "foreground": "error"});
    assert_eq!(preview["segments"], json!([["model", second]]));
    keys(&mut editor, &[ENTER, ESC, DOWN, ENTER]);
    let saved = saved.lock().unwrap();
    assert_eq!(
        saved[0],
        json!({"theme": "dracula", "type": "builtin", "segments": [["model", second]]})
    );
}
