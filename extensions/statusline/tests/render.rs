mod support;

use support::*;

#[test]
fn parses_command_and_builtin_settings() {
    let command = settings(
        json!({"type": "command", "command": "~/s.sh", "padding": 2, "refreshInterval": 0.5}),
    );
    let Some(StatusLineConfig::Command(config)) =
        StatusLineConfig::from_settings(&command, Path::new(HOME))
    else {
        panic!("expected a command status line");
    };
    assert_eq!(
        (config.command.as_str(), config.padding, config.refresh),
        ("~/s.sh", 2, None)
    );
    let refresh = settings(json!({"type": "command", "command": "s.sh", "refreshInterval": 5}));
    let Some(StatusLineConfig::Command(config)) =
        StatusLineConfig::from_settings(&refresh, Path::new(HOME))
    else {
        panic!("expected a command status line");
    };
    assert_eq!(config.refresh, Some(Duration::from_secs(5)));
    for value in [
        json!({"type": "command"}),
        json!({"type": "rainbow"}),
        json!({"type": "off"}),
    ] {
        assert_eq!(
            StatusLineConfig::from_settings(&settings(value), Path::new(HOME)),
            None
        );
    }

    let minimal = builtin(json!({"type": "builtin"}));
    assert_eq!(
        (
            minimal.chip,
            minimal.palette.clone(),
            minimal.separator.as_str()
        ),
        (Chip::Plain, None, " • ")
    );
    let custom = builtin(
        json!({"preset": "powerline", "chip": "rounded", "theme": "nord", "icons": "emoji",
        "segments": ["model", "nope", "git-branch", {"text": "λ", "color": "#ff0000"}, {"segment": "cost", "icon": "$"}]}),
    );
    assert_eq!(
        (custom.chip, custom.palette, custom.icons),
        (Chip::Rounded, Palette::named("nord"), Icons::Emoji)
    );
    let kinds: Vec<ItemKind> = custom.lines[0]
        .iter()
        .map(|item| item.kind.clone())
        .collect();
    assert_eq!(
        kinds,
        [
            ItemKind::Segment(Segment::Model),
            ItemKind::Segment(Segment::Branch),
            ItemKind::Text("λ".into()),
            ItemKind::Segment(Segment::Cost)
        ]
    );
    assert_eq!(custom.lines[0][2].color.as_deref(), Some("#ff0000"));
    assert_eq!(custom.lines[0][3].icon.as_deref(), Some("$"));
    assert_eq!(builtin(json!({"style": "pills"})).chip, Chip::Block);
    let two_lines = builtin(json!({"segments": [["cwd", "branch"], ["tokens"], []]}));
    assert_eq!(two_lines.lines.len(), 2);
}

#[test]
fn every_preset_theme_and_chip_draws_a_line_that_fits() {
    for preset in preset_names() {
        for width in [120, 30] {
            let lines = builtin(json!({"preset": preset})).render(&snapshot(), width);
            assert!(!lines.is_empty(), "{preset}");
            for line in &lines {
                assert!(
                    visible_width(line) <= width,
                    "{preset} at {width}: {line:?}"
                );
            }
        }
    }
    for theme in theme_names() {
        assert!(Palette::named(theme).is_some(), "{theme}");
        let line = first_line(
            json!({"chip": "block", "theme": theme, "segments": ["dir", "model"]}),
            60,
        );
        assert!(
            line.contains("\x1b[48;"),
            "{theme} draws backgrounds: {line:?}"
        );
    }
    for chip in [
        "plain",
        "block",
        "rounded",
        "powerline",
        "slanted",
        "brackets",
        "dot",
    ] {
        let line = first_line(
            json!({"chip": chip, "theme": "dracula", "segments": ["dir", "fill", "model"]}),
            60,
        );
        assert_eq!(visible_width(&line), 60, "{chip}: {line:?}");
        assert!(plain_line(&line).contains("node-cli"), "{chip}");
    }
}

#[test]
fn chips_have_their_shapes() {
    let segments = json!(["dir", "fill", "model"]);
    let shape = |chip: &str| {
        plain_line(&first_line(
            json!({"chip": chip, "icons": "none", "segments": segments}),
            40,
        ))
    };
    assert!(
        shape("brackets").starts_with("[node-cli]") && shape("brackets").ends_with("[gpt-6.1-sol]")
    );
    assert!(shape("dot").starts_with("● node-cli") && shape("dot").ends_with("● gpt-6.1-sol"));
    assert!(shape("block").starts_with(" node-cli ") && shape("block").ends_with(" gpt-6.1-sol "));
    assert!(shape("rounded").starts_with("\u{e0b6} node-cli \u{e0b4}"));
    assert!(
        shape("powerline").starts_with(" node-cli \u{e0b0}")
            && shape("powerline").contains("\u{e0b2} gpt")
    );
    assert!(
        shape("slanted").starts_with(" node-cli \u{e0bc}")
            && shape("slanted").contains("\u{e0ba} gpt")
    );
}

#[test]
fn a_long_right_side_is_cut_and_the_chip_picks_the_separator() {
    let line = plain_line(&first_line(json!({"preset": "cursor"}), 40));
    assert!(
        line.starts_with("gpt-6.1-sol · 3% used ") && line.ends_with('…'),
        "{line:?}"
    );
    assert_eq!(visible_width(&line), 40);
    let block = plain_line(&first_line(
        json!({"chip": "block", "icons": "none", "segments": ["dir", "model"]}),
        60,
    ));
    assert!(block.starts_with(" node-cli   gpt-6.1-sol "), "{block:?}");
}

#[test]
fn minimal_preset_puts_the_model_left_and_the_directory_right() {
    let line = plain_line(&first_line(json!({"type": "builtin"}), 80));
    assert!(
        line.starts_with("gpt-6.1-sol • high • 2.7%/500k (auto) • $0.013"),
        "{line:?}"
    );
    assert!(
        line.ends_with("node-cli • ⎇ format-double-quotes"),
        "{line:?}"
    );
    assert_eq!(visible_width(&line), 80);
}

#[test]
fn segments_show_what_the_footer_shows() {
    let text = |segment: Segment| segment_text(segment, &snapshot());
    let cwd = format!(
        "~{}Workspace{}node-cli",
        std::path::MAIN_SEPARATOR,
        std::path::MAIN_SEPARATOR
    );
    assert_eq!(text(Segment::Cwd), Some(cwd));
    assert_eq!(text(Segment::Project).as_deref(), Some("node-cli"));
    assert_eq!(text(Segment::SessionId).as_deref(), Some("0f3c9a7e"));
    assert_eq!(
        text(Segment::ModelWithReasoning).as_deref(),
        Some("gpt-6.1-sol high")
    );
    assert_eq!(text(Segment::Tokens).as_deref(), Some("↑5.1k ↓193"));
    assert_eq!(text(Segment::TokensUsed).as_deref(), Some("9.8k used"));
    assert_eq!(text(Segment::Cache).as_deref(), Some("R4.5k CH90.6%"));
    assert_eq!(text(Segment::ContextUsed).as_deref(), Some("3% used"));
    assert_eq!(text(Segment::ContextRemaining).as_deref(), Some("97% left"));
    assert_eq!(text(Segment::ContextWindow).as_deref(), Some("500k window"));
    assert_eq!(text(Segment::ContextBar).as_deref(), Some("▱▱▱▱▱▱▱▱ 3%"));
    assert_eq!(text(Segment::RunState).as_deref(), Some("● working"));
    assert_eq!(text(Segment::Logo).as_deref(), Some("π"));
    assert_eq!(text(Segment::Version).as_deref(), Some("v0.8.0"));
    assert_eq!(text(Segment::Time).map(|time| time.len()), Some(5));
    assert_eq!(text(Segment::Fill), None);
    let mut full = snapshot();
    full.context = Some(ContextUsage {
        tokens: None,
        context_window: 500_000,
        percent: Some(95.0),
    });
    full.model.as_mut().expect("model").reasoning = false;
    assert_eq!(segment_text(Segment::Thinking, &full), None);
    assert_eq!(
        segment_text(Segment::ContextBar, &full).as_deref(),
        Some("▰▰▰▰▰▰▰▰ 95%")
    );
    assert_eq!(Segment::ContextBar.role(&full), Role::ContextHigh);
    for segment in Segment::ALL {
        assert_eq!(Segment::from_name(segment.name()), Some(*segment));
    }
}
