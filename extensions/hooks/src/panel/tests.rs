use pi_ext_sdk::Key;
use pi_ext_sdk::style;
use serde_json::json;

use super::*;
use crate::config::options::SWITCHES;
use crate::model::{Layer, Plugin};

fn def(
    source: Source,
    scope: Scope,
    file: &str,
    event: &str,
    matcher: Option<&str>,
    cmd: &str,
) -> HookDef {
    let handler = json!({ "type": "command", "command": cmd, "timeout": 30 });
    HookDef::new(
        source,
        scope,
        file.into(),
        event,
        matcher.map(str::to_owned),
        handler,
    )
}

fn rows() -> Vec<Row> {
    let mut plugin = def(
        Source::Pi,
        Scope::Global,
        "/a/ext/hooks.json",
        "PostToolUse",
        Some("Edit|Write"),
        "${CLAUDE_PLUGIN_ROOT}/fmt.sh",
    );
    plugin.plugin = Some(Plugin {
        id: Some("formatter@acme".into()),
        root: None,
        data: None,
        env: Vec::new(),
    });
    let mut rows = vec![
        Row::new(
            def(
                Source::Claude,
                Scope::Global,
                "/h/.claude/settings.json",
                "PreToolUse",
                Some("Bash"),
                "~/.claude/guard.sh --strict",
            ),
            true,
            Trust::Trusted,
            None,
        ),
        Row::new(
            HookDef {
                layer: Layer::Local,
                ..def(
                    Source::Claude,
                    Scope::Project,
                    "/r/.claude/settings.local.json",
                    "Stop",
                    None,
                    "say done",
                )
            },
            false,
            Trust::Untrusted,
            None,
        ),
        Row::new(
            def(
                Source::Codex,
                Scope::Project,
                "/r/.codex/hooks.json",
                "UserPromptSubmit",
                None,
                "./scripts/context.sh",
            ),
            true,
            Trust::Denied,
            None,
        ),
        Row::new(
            def(
                Source::Pi,
                Scope::Project,
                "/r/.pi/ext/hooks.json",
                "SessionStart",
                Some("startup"),
                "git status --short",
            ),
            true,
            Trust::Once,
            None,
        ),
        Row::new(plugin, true, Trust::All, None),
    ];
    sort_rows(&mut rows);
    rows
}

fn press(view: &mut View, rows: &[Row], keys: &[&str]) -> Action {
    keys.iter()
        .map(|k| view.key(&Key::parse(k), rows))
        .last()
        .unwrap_or(Action::Redraw)
}

fn plain(lines: &[String]) -> Vec<String> {
    lines.iter().map(|l| style::strip(l)).collect()
}

const DOWN: &str = "\x1b[B";
const UP: &str = "\x1b[A";
const RIGHT: &str = "\x1b[C";
const LEFT: &str = "\x1b[D";
const ESC: &str = "\x1b";

#[test]
fn renders_within_40_and_100_columns() {
    let rows = rows();
    for width in [40, 100] {
        for keys in [&[][..], &["\r"], &[DOWN, "\r"], &["b"], &["\t", "\t", "\t"]] {
            let mut view = View::new(true);
            press(&mut view, &rows, keys);
            for line in view.render(&rows, width, None) {
                assert!(style::visible_width(&line) <= width, "{width}: {line:?}");
            }
        }
    }
    let view = View::new(true);
    let wide = plain(&view.render(&rows, 100, None)).join("\n");
    let settings = format!("Settings ({})", Setting::all().len());
    for text in [
        "[All (5)]",
        &settings,
        "+ Add a hook",
        "Claude Code (2)",
        "Codex (1)",
        "Plugins (1)",
        "PreToolUse",
        "local",
        "untrusted",
        "denied",
        "~/.claude/guard.sh --strict",
        "type to search",
        "←→ on/off",
        "enter actions",
        "esc close",
    ] {
        assert!(wide.contains(text), "missing {text}:\n{wide}");
    }
    let narrow = plain(&view.render(&rows, 40, None));
    assert!(
        narrow.iter().any(|l| l.contains("● PreToolUse Bash G✓")),
        "{narrow:#?}"
    );
    assert!(
        narrow.iter().any(|l| l.contains("○ Stop * L?")),
        "{narrow:#?}"
    );
    let footer = narrow.join(" ");
    for key in ["enter actions", "←→ on/off", "type to search", "esc close"] {
        assert!(footer.contains(key), "missing {key}: {footer}");
    }
}

#[test]
fn details_show_command_timeout_flags_and_file() {
    let rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &[DOWN, "\r"]);
    let text = plain(&view.render(&rows, 40, None)).join("\n");
    for part in [
        "PreToolUse · Bash",
        "Claude Code, global",
        "/h/.claude/settings.json",
        "30s",
        "trusted",
        "Actions",
        "→ Disable",
        "Remove from file",
    ] {
        assert!(text.contains(part), "missing {part}:\n{text}");
    }
}

#[test]
fn moves_and_pages() {
    let rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &[UP]);
    assert_eq!(view.selected, 5);
    press(&mut view, &rows, &[DOWN]);
    assert_eq!(view.selected, 0);
    press(&mut view, &rows, &["\x1b[F"]);
    assert_eq!(view.selected, 5);
    press(&mut view, &rows, &["\x1b[H", "\x1b[6~"]);
    assert_eq!(view.selected, 5);
    press(&mut view, &rows, &["\x1b[5~"]);
    assert_eq!(view.selected, 0);
}

#[test]
fn arrows_toggle_and_enter_opens_actions_and_remove_confirms() {
    let rows = rows();
    let first = rows[0].def.hash.clone();
    let mut view = View::new(true);
    assert_eq!(
        press(&mut view, &rows, &[DOWN, RIGHT]),
        Action::Toggle(first.clone())
    );
    assert_eq!(
        press(&mut view, &rows, &[LEFT]),
        Action::Toggle(first.clone())
    );
    assert_eq!(press(&mut view, &rows, &[" "]), Action::Redraw);
    assert_eq!(view.query, " ");
    press(&mut view, &rows, &[ESC]);
    assert!(view.query.is_empty());
    assert_eq!(view.selected, 0);
    assert_eq!(
        press(&mut view, &rows, &[DOWN, "\r", "\r"]),
        Action::Toggle(first.clone())
    );
    assert_eq!(view.screen, Screen::List);
    assert_eq!(
        press(&mut view, &rows, &["\r", DOWN, "\r"]),
        Action::Trust(first.clone(), TrustChange::Always)
    );
    press(&mut view, &rows, &["\r", ESC]);
    assert_eq!(view.screen, Screen::List);
    press(&mut view, &rows, &["\r", UP, "\r"]);
    assert_eq!(
        view.screen,
        Screen::Confirm {
            hash: first.clone()
        }
    );
    let text = plain(&view.render(&rows, 80, None)).join("\n");
    assert!(text.contains("Enter removes it"), "{text}");
    assert_eq!(press(&mut view, &rows, &["n"]), Action::Redraw);
    assert_eq!(view.status, Some((Tone::Info, "Cancelled.".into())));
    assert_eq!(
        press(&mut view, &rows, &["\r", UP, "\r", "\r"]),
        Action::Remove(first)
    );
    assert_eq!(press(&mut view, &rows, &[ESC]), Action::Close);
}

#[test]
fn tabs_filter_by_scope_and_typing_searches() {
    let rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &["\t"]);
    assert_eq!(view.tab, Tab::Project);
    assert_eq!(view.entries(&rows).len(), 4);
    press(&mut view, &rows, &["\t"]);
    assert_eq!(view.entries(&rows).len(), 3);
    press(&mut view, &rows, &["\t"]);
    assert_eq!(view.tab, Tab::Settings);
    assert_eq!(view.entries(&rows).len(), Setting::all().len());
    press(&mut view, &rows, &["\x1b[Z", "\x1b[Z", "\x1b[Z"]);
    assert_eq!(view.tab, Tab::All);
    press(&mut view, &rows, &["q", "\x7f", "c", "o", "d", "e", "x"]);
    assert_eq!(view.query, "codex");
    assert_eq!(view.entries(&rows).len(), 1);
    assert_eq!(press(&mut view, &rows, &[ESC]), Action::Redraw);
    assert!(view.query.is_empty());
    press(&mut view, &rows, &["plugins"]);
    assert_eq!(view.entries(&rows).len(), 1);
}

#[test]
fn settings_change_in_place_and_from_the_actions_pane() {
    let rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &["\x1b[Z"]);
    assert_eq!(view.tab, Tab::Settings);
    let claude = Setting::Source(Source::Claude);
    assert_eq!(press(&mut view, &rows, &[RIGHT]), Action::Set(claude, 1));
    assert_eq!(press(&mut view, &rows, &[LEFT]), Action::Set(claude, 1));
    press(&mut view, &rows, &["\x1b[F"]);
    press(&mut view, &rows, &vec![UP; SWITCHES.len()]);
    assert_eq!(
        press(&mut view, &rows, &[RIGHT]),
        Action::Set(Setting::Conflicts, 1)
    );
    assert_eq!(
        press(&mut view, &rows, &[LEFT]),
        Action::Set(Setting::Conflicts, 2)
    );
    press(&mut view, &rows, &["\r"]);
    assert_eq!(view.screen, Screen::Menu { choice: 0 });
    let text = plain(&view.render(&rows, 100, None)).join("\n");
    assert!(
        text.contains("merge: run the hooks of all layers"),
        "{text}"
    );
    assert!(text.contains("(current)"), "{text}");
    assert_eq!(
        press(&mut view, &rows, &[DOWN, DOWN, "\r"]),
        Action::Set(Setting::Conflicts, 2)
    );
    assert_eq!(Setting::Conflicts.json(2), json!("override"));
    view.options.conflicts = crate::policy::ConflictMode::Override;
    let text = plain(&view.render(&rows, 100, None)).join("\n");
    assert!(text.contains("conflicts: override"), "{text}");
    assert!(
        text.contains("Conflicts between layers  override"),
        "{text}"
    );
}

#[test]
fn add_form_runs_in_the_panel() {
    let rows = rows();
    let mut view = View::new(false);
    press(
        &mut view,
        &rows,
        &["\r", DOWN, "\r", "B", "a", "s", "h", "\r"],
    );
    press(&mut view, &rows, &["e", "c", "h", "o", " ", "q", "\t"]);
    let text = plain(&view.render(&rows, 40, None)).join("\n");
    assert!(text.contains("Add a command hook"), "{text}");
    assert!(text.contains("echo q"), "{text}");
    assert!(!text.contains("tab scope"), "{text}");
    assert_eq!(
        press(&mut view, &rows, &["\r"]),
        Action::Add {
            scope: Scope::Global,
            event: crate::manage::EVENTS[1].to_owned(),
            matcher: "Bash".into(),
            command: "echo q".into(),
        }
    );
    let mut view = View::new(true);
    press(&mut view, &rows, &["\t", "\r"]);
    assert!(matches!(&view.screen, Screen::Add(form) if form.scope == Scope::Project));
    press(&mut view, &rows, &["\t"]);
    assert!(matches!(&view.screen, Screen::Add(form) if form.scope == Scope::Global));
    press(&mut view, &rows, &["\r", "\r", "\r"]);
    assert!(matches!(&view.screen, Screen::Add(form) if form.step == AddStep::Command));
    press(&mut view, &rows, &["\x7f", "\x7f"]);
    assert!(matches!(&view.screen, Screen::Add(form) if form.step == AddStep::Event));
    press(&mut view, &rows, &[ESC]);
    assert_eq!(view.screen, Screen::List);
}

#[test]
fn managed_hooks_show_details_without_actions() {
    let mut rows = rows();
    rows[0].def.layer = Layer::Managed;
    rows[0].trust = Trust::Managed;
    let mut view = View::new(true);
    assert_eq!(press(&mut view, &rows, &[DOWN, RIGHT]), Action::Redraw);
    let status = view.status.clone().unwrap();
    assert_eq!(status.0, Tone::Warn);
    assert!(status.1.contains("managed hook"), "{status:?}");
    assert_eq!(press(&mut view, &rows, &["\r", "\r"]), Action::Redraw);
    press(&mut view, &rows, &["\r"]);
    let text = plain(&view.render(&rows, 80, None)).join("\n");
    assert!(!text.contains("Actions"), "{text}");
    assert!(text.contains("policy file"), "{text}");
}

#[test]
fn clamp_leaves_screens_of_removed_rows() {
    let mut rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &["\x1b[F", "\r", UP, "\r"]);
    assert!(matches!(view.screen, Screen::Confirm { .. }));
    rows.pop();
    view.clamp(&rows);
    assert_eq!(view.screen, Screen::List);
    assert_eq!(view.selected, rows.len());
}
