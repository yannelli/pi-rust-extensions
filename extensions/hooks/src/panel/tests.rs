use pi_ext_sdk::Key;
use pi_ext_sdk::style;
use serde_json::json;

use super::*;
use crate::model::Plugin;

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
            def(
                Source::Claude,
                Scope::Project,
                "/r/.claude/settings.local.json",
                "Stop",
                None,
                "say done",
            ),
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

#[test]
fn renders_within_40_and_100_columns() {
    let rows = rows();
    let mut view = View::new(true);
    for width in [40, 100] {
        for keys in [&[][..], &["\r"], &["d"], &["a"], &["/", "b"]] {
            view.mode = Mode::List;
            view.query.clear();
            press(&mut view, &rows, keys);
            for line in view.render(&rows, width) {
                assert!(style::visible_width(&line) <= width, "{width}: {line:?}");
            }
        }
    }
    view.mode = Mode::List;
    view.query.clear();
    let wide = plain(&view.render(&rows, 100)).join("\n");
    for text in [
        "[All (5)]",
        "Claude Code (2)",
        "Codex (1)",
        "Plugins (1)",
        "PreToolUse",
        "local",
        "untrusted",
        "denied",
        "~/.claude/guard.sh --strict",
        "space toggle",
        "esc close",
    ] {
        assert!(wide.contains(text), "missing {text}:\n{wide}");
    }
    let narrow = plain(&view.render(&rows, 40));
    assert!(
        narrow.iter().any(|l| l.contains("● PreToolUse Bash G✓")),
        "{narrow:#?}"
    );
    assert!(
        narrow.iter().any(|l| l.contains("○ Stop * L?")),
        "{narrow:#?}"
    );
    let footer = narrow.join(" ");
    for key in [
        "space toggle",
        "d remove",
        "a add",
        "/ search",
        "tab filter",
        "esc close",
    ] {
        assert!(footer.contains(key), "missing {key}: {footer}");
    }
    println!("{}", plain(&view.render(&rows, 100)).join("\n"));
}

#[test]
fn details_show_command_timeout_flags_and_file() {
    let rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &["\r"]);
    let text = plain(&view.render(&rows, 40)).join("\n");
    for part in [
        "PreToolUse · Bash",
        "Claude Code, global",
        "/h/.claude/settings.json",
        "30s",
        "trusted",
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
    press(&mut view, &rows, &["\x1b[A"]);
    assert_eq!(view.selected, 4);
    press(&mut view, &rows, &["\x1b[B"]);
    assert_eq!(view.selected, 0);
    press(&mut view, &rows, &["\x1b[F"]);
    assert_eq!(view.selected, 4);
    press(&mut view, &rows, &["\x1b[H", "\x1b[6~"]);
    assert_eq!(view.selected, 4);
    press(&mut view, &rows, &["\x1b[5~"]);
    assert_eq!(view.selected, 0);
}

#[test]
fn toggles_details_and_removes() {
    let rows = rows();
    let first = rows[0].def.hash.clone();
    let mut view = View::new(true);
    assert_eq!(
        press(&mut view, &rows, &[" "]),
        Action::Toggle(first.clone())
    );
    assert_eq!(
        press(&mut view, &rows, &["\r", "\r"]),
        Action::Toggle(first.clone())
    );
    assert_eq!(
        press(&mut view, &rows, &["\x1b[B", "\r"]),
        Action::Trust(first.clone(), TrustChange::Always)
    );
    assert_eq!(press(&mut view, &rows, &["\x1b"]), Action::Redraw);
    assert_eq!(view.mode, Mode::List);
    press(&mut view, &rows, &["d"]);
    assert_eq!(
        view.mode,
        Mode::ConfirmRemove {
            hash: first.clone()
        }
    );
    assert_eq!(press(&mut view, &rows, &["n"]), Action::Redraw);
    assert_eq!(
        press(&mut view, &rows, &["\x1b[3~", "y"]),
        Action::Remove(first)
    );
    assert_eq!(press(&mut view, &rows, &["\x1b"]), Action::Close);
}

#[test]
fn tabs_filter_by_scope_and_search_types() {
    let rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &["\t"]);
    assert_eq!(view.tab, Tab::Project);
    assert_eq!(view.visible(&rows).len(), 3);
    press(&mut view, &rows, &["\t"]);
    assert_eq!(view.visible(&rows).len(), 2);
    press(&mut view, &rows, &["\x1b[Z", "\x1b[Z"]);
    assert_eq!(view.tab, Tab::All);
    assert_eq!(
        press(
            &mut view,
            &rows,
            &["/", "q", "\x7f", "c", "o", "d", "e", "x"]
        ),
        Action::Redraw
    );
    assert_eq!(view.query, "codex");
    assert_eq!(view.visible(&rows).len(), 1);
    press(&mut view, &rows, &["\r"]);
    assert_eq!(view.mode, Mode::List);
    assert_eq!(press(&mut view, &rows, &["\x1b"]), Action::Redraw);
    assert!(view.query.is_empty());
    press(&mut view, &rows, &["/", "plugins"]);
    assert_eq!(view.visible(&rows).len(), 1);
}

#[test]
fn add_asks_for_scope_and_refuses_untrusted_project() {
    let rows = rows();
    let mut view = View::new(false);
    assert_eq!(press(&mut view, &rows, &["a", "p"]), Action::Redraw);
    assert!(
        view.status
            .as_deref()
            .is_some_and(|s| s.contains("not trusted"))
    );
    let text = plain(&view.render(&rows, 40)).join("\n");
    assert!(text.contains("not trusted"), "{text}");
    assert_eq!(
        press(&mut view, &rows, &["a", "g"]),
        Action::Add(Scope::Global)
    );
    let mut view = View::new(true);
    assert_eq!(
        press(&mut view, &rows, &["a", "p"]),
        Action::Add(Scope::Project)
    );
    assert_eq!(press(&mut view, &rows, &["a", "\x1b"]), Action::Redraw);
    assert_eq!(view.mode, Mode::List);
}

#[test]
fn clamp_leaves_details_of_removed_rows() {
    let mut rows = rows();
    let mut view = View::new(true);
    press(&mut view, &rows, &["\x1b[F", "\r"]);
    rows.pop();
    view.clamp(&rows);
    assert_eq!(view.mode, Mode::List);
    assert_eq!(view.selected, 3);
}
