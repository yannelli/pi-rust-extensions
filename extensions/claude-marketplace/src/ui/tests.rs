use pi_ext_sdk::{Key, style};

use super::{Action, Inventory, Market, Op, Plugin, Screen, Tab, Tone, View};
use crate::env::Scope;
use crate::listing::{PluginRow, Status};

fn plugin(name: &str, market: &str, enabled: Option<bool>, status: Status) -> Plugin {
    Plugin {
        row: PluginRow {
            marketplace: market.into(),
            scope: Scope::User,
            name: name.into(),
            status,
            version: Some("1.2.0".into()),
            description: Some(format!("What {name} does")),
            reasons: if status == Status::Unavailable && enabled.is_none() {
                vec!["npm plugin sources are not supported".into()]
            } else {
                Vec::new()
            },
            enabled,
        },
        commands: 2,
        skills: 1,
        agents: 0,
        hooks: true,
        mcp: Vec::new(),
    }
}

fn inventory() -> Inventory {
    Inventory {
        plugins: vec![
            plugin("helper", "acme", Some(true), Status::Installed),
            plugin("quiet", "acme", Some(false), Status::Unavailable),
            plugin("fresh", "acme", None, Status::Available),
            plugin("npm-one", "acme", None, Status::Unavailable),
        ],
        markets: vec![Market {
            name: "acme".into(),
            scope: Scope::User,
            source: "acme/plugins".into(),
            plugins: 4,
            installed: 2,
            auto_update: false,
            git: true,
        }],
        error: None,
    }
}

fn names(view: &View, inv: &Inventory) -> Vec<String> {
    view.entries(inv)
        .iter()
        .map(|entry| match entry {
            super::Entry::Plugin(p) => p.id(),
            super::Entry::Market(m) => m.name.clone(),
            super::Entry::Special(s) => format!("{s:?}"),
        })
        .collect()
}

#[test]
fn tabs_split_installed_discover_and_marketplaces() {
    let inv = inventory();
    let mut view = View::new(true);
    assert_eq!(names(&view, &inv), ["helper@acme", "quiet@acme"]);
    view.key(&Key::Tab, &inv);
    assert_eq!(view.tab(), Tab::Discover);
    assert_eq!(
        names(&view, &inv),
        ["fresh@acme", "npm-one@acme"],
        "installable plugins first"
    );
    view.key(&Key::Tab, &inv);
    assert_eq!(names(&view, &inv), ["Add", "UpdateAll", "Import", "acme"]);
    view.key(&Key::BackTab, &inv);
    assert_eq!(view.tab(), Tab::Discover);
}

#[test]
fn typing_filters_and_escape_clears_before_closing() {
    let inv = inventory();
    let mut view = View::new(false);
    for c in "qui".chars() {
        view.key(&Key::Char(c), &inv);
    }
    assert_eq!(names(&view, &inv), ["quiet@acme"]);
    assert_eq!(view.key(&Key::Escape, &inv), Action::Redraw);
    assert!(view.query.is_empty());
    assert_eq!(view.key(&Key::Escape, &inv), Action::Close);
}

#[test]
fn arrows_toggle_plugins_and_auto_update_inline() {
    let inv = inventory();
    let mut view = View::new(true);
    assert_eq!(
        view.key(&Key::Right, &inv),
        Action::Run(Op::SetEnabled {
            id: "helper@acme".into(),
            scope: Scope::User,
            enabled: false
        })
    );
    view.key(&Key::Down, &inv);
    assert_eq!(
        view.key(&Key::Left, &inv),
        Action::Run(Op::SetEnabled {
            id: "quiet@acme".into(),
            scope: Scope::User,
            enabled: true
        })
    );
    view.tab = 2;
    view.selected = 3;
    assert_eq!(
        view.key(&Key::Right, &inv),
        Action::Run(Op::AutoUpdate {
            name: "acme".into(),
            on: true
        })
    );
}

#[test]
fn enter_opens_actions_and_uninstall_asks_first() {
    let inv = inventory();
    let mut view = View::new(true);
    view.key(&Key::Enter, &inv);
    assert_eq!(view.screen, Screen::Menu { choice: 0 });
    let labels: Vec<String> = view.choices(&inv).into_iter().map(|c| c.label).collect();
    assert_eq!(
        labels,
        ["Turn off", "Update", "Configure", "Reinstall", "Uninstall"]
    );
    for _ in 0..4 {
        view.key(&Key::Down, &inv);
    }
    assert_eq!(view.key(&Key::Enter, &inv), Action::Redraw);
    assert!(matches!(view.screen, Screen::Confirm { .. }));
    assert_eq!(
        view.key(&Key::Enter, &inv),
        Action::Run(Op::Uninstall {
            id: "helper@acme".into(),
            scope: Scope::User
        })
    );

    // Any other key cancels.
    view.key(&Key::Enter, &inv);
    for _ in 0..4 {
        view.key(&Key::Down, &inv);
    }
    view.key(&Key::Enter, &inv);
    assert_eq!(view.key(&Key::Char('n'), &inv), Action::Redraw);
    assert_eq!(view.screen, Screen::List);
}

#[test]
fn installing_offers_the_project_only_when_trusted_and_explains_unavailable_plugins() {
    let inv = inventory();
    for (trusted, expected) in [(true, 2), (false, 1)] {
        let mut view = View::new(trusted);
        view.tab = 1;
        view.key(&Key::Enter, &inv);
        assert_eq!(view.choices(&inv).len(), expected);
        assert_eq!(
            view.key(&Key::Enter, &inv),
            Action::Run(Op::Install {
                id: "fresh@acme".into(),
                scope: Scope::User
            })
        );
    }
    let mut view = View::new(true);
    view.tab = 1;
    view.selected = 1;
    view.key(&Key::Enter, &inv);
    assert_eq!(view.screen, Screen::List);
    let (tone, text) = view.status.clone().unwrap();
    assert_eq!(tone, Tone::Warn);
    assert!(text.contains("npm plugin sources"), "{text}");
}

#[test]
fn adding_a_marketplace_is_an_input_line_in_the_panel() {
    let inv = inventory();
    let mut view = View::new(true);
    view.tab = 2;
    view.key(&Key::Enter, &inv);
    assert!(matches!(view.screen, Screen::Add { .. }));
    view.key(
        &Key::Text("anthropics/claude-plugins-official".into()),
        &inv,
    );
    view.key(&Key::Tab, &inv);
    assert_eq!(
        view.key(&Key::Enter, &inv),
        Action::Run(Op::AddMarket {
            source: "anthropics/claude-plugins-official".into(),
            scope: Scope::Project
        })
    );
    view.tab = 2;
    view.selected = 1;
    assert_eq!(view.key(&Key::Enter, &inv), Action::Run(Op::UpdateAll));
}

#[test]
fn renders_fit_the_panel_and_show_progress() {
    let inv = inventory();
    let mut view = View::new(true);
    for (width, height) in [(60, Some(14)), (120, Some(30)), (30, None)] {
        let lines = view.render(&inv, width, height);
        assert!(
            lines
                .iter()
                .all(|line| style::visible_width(line) <= width.max(24)),
            "{lines:?}"
        );
        if let Some(height) = height {
            assert!(lines.len() <= height);
        }
    }
    let text = style::strip(&view.render(&inv, 100, Some(30)).join("\n"));
    assert!(
        text.contains("[Installed (2)]")
            && text.contains("Discover (2)")
            && text.contains("Marketplaces (1)"),
        "{text}"
    );
    assert!(
        text.contains("helper@acme") && text.contains("enter actions"),
        "{text}"
    );
    view.key(&Key::Enter, &inv);
    let details = style::strip(&view.render(&inv, 100, Some(30)).join("\n"));
    assert!(details.contains("1 skill, 2 commands, hooks"), "{details}");
    view.status = Some((Tone::Busy, "Updating acme (pulling)… 3s".into()));
    view.frame = 2;
    let busy = style::strip(&view.render(&inv, 100, Some(30)).join("\n"));
    assert!(busy.contains("⠹ Updating acme (pulling)… 3s"), "{busy}");
}
