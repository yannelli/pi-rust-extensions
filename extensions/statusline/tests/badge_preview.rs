use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use pi_ext_statusline::builtin::BuiltinStatusLine;
use pi_ext_statusline::config::StatusLineSettings;
use pi_ext_statusline::editor::StatusLineEditor;
use pi_ext_statusline::model::StatusSnapshot;
use serde_json::{Value, json};

const ENTER: &str = "\r";
const ESC: &str = "\x1b";
const DOWN: &str = "\x1b[B";

fn project_editor() -> (StatusLineEditor, Arc<Mutex<Vec<Value>>>) {
    let saved = Arc::new(Mutex::new(Vec::new()));
    let writes = saved.clone();
    let save = Box::new(move |value| {
        writes.lock().unwrap().push(value);
        Ok(())
    });
    let global = json!({"type": "builtin", "chip": "block", "theme": "ocean",
        "icons": "none", "segments": [[{"text": "ALPHA"}, {"text": "BETA"}]]});
    let editor = StatusLineEditor::new(
        Some(json!({"theme": "forest"})),
        PathBuf::from("/nonexistent-home"),
        save,
    )
    .with_badge_defaults(global);
    (editor, saved)
}

fn keys(editor: &mut StatusLineEditor, data: &[&str]) {
    for key in data {
        editor.handle_input(key);
    }
}

#[test]
fn applying_project_badge_colors_keeps_the_inherited_shape_in_the_live_preview() {
    let (mut editor, saved) = project_editor();
    editor.select_item("badges");
    keys(
        &mut editor,
        &[
            ENTER, ENTER, ENTER, "#ffffff", ENTER, ESC, DOWN, DOWN, ENTER,
        ],
    );
    let preview = editor.preview_value();
    assert_eq!(preview["chip"], "block");
    assert_eq!(preview["theme"], "forest");
    assert_eq!(preview["segments"][0][0]["foreground"], "#ffffff");
    let settings: StatusLineSettings = serde_json::from_value(preview).unwrap();
    let lines =
        BuiltinStatusLine::from_settings(&settings, 0).render(&StatusSnapshot::default(), 60);
    assert!(lines[0].contains("\x1b[48;"), "{lines:?}");
    assert!(lines[0].contains("ALPHA") && lines[0].contains("BETA"));
    assert_eq!(
        saved.lock().unwrap().as_slice(),
        &[json!({"type": "builtin", "theme": "forest",
        "segments": [[{"text": "ALPHA", "foreground": "#ffffff"}, {"text": "BETA"}]]})]
    );
}

#[test]
fn canceling_project_badge_colors_restores_the_effective_saved_footer() {
    let (mut editor, saved) = project_editor();
    editor.select_item("badges");
    keys(&mut editor, &[ENTER, ENTER, ENTER, "#ffffff"]);
    assert_eq!(
        editor.preview_value()["segments"][0][0]["foreground"],
        "#ffffff"
    );
    keys(&mut editor, &[ESC, ESC, ESC]);
    assert_eq!(
        editor.preview_value(),
        json!({"type": "builtin", "theme": "forest",
        "chip": "block", "icons": "none", "segments": [[{"text": "ALPHA"}, {"text": "BETA"}]]})
    );
    assert_eq!(editor.value, json!({"theme": "forest"}));
    assert!(saved.lock().unwrap().is_empty());
}
