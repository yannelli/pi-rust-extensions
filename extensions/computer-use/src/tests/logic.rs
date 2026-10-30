use serde_json::json;

use crate::act::UiAction;
use crate::config::{self, ManagedBrowser, Partial, SourceStatus};
use crate::live::{FindArgs, select_roots};
use crate::outline::{Node, Outline, Rect, normalize_role, search};
use crate::output;
use crate::state::{Capture, Observation, Root, STATE_CAP, Store, Target};

pub fn leaf(role: &str, label: &str, path: usize) -> Node {
    let frame = Some(Rect {
        x: 10.0 * path as f64,
        y: 0.0,
        w: 10.0,
        h: 10.0,
    });
    Node {
        role: role.into(),
        label: Some(label.into()),
        path: vec![path],
        frame,
        ..Node::default()
    }
}

pub fn observation(state_id: &str) -> Observation {
    let mut save = leaf("button", "Save", 0);
    save.can_press = true;
    let mut title = leaf("text_field", "Title", 1);
    title.can_set_value = true;
    title.can_focus = true;
    let mut root = Node {
        role: "window".into(),
        label: Some("Notes".into()),
        children: vec![save, title],
        ..Node::default()
    };
    root.assign_refs(&mut 0);
    Observation {
        target: Target {
            app: "Notes".into(),
            pid: 1,
            window_id: 2,
            frame_points: Rect {
                x: 100.0,
                y: 100.0,
                w: 400.0,
                h: 300.0,
            },
            ..Target::default()
        },
        capture: Capture {
            state_id: state_id.into(),
            width: 800,
            height: 600,
            ..Capture::default()
        },
        outline: Outline {
            look_id: "l".into(),
            root,
        },
    }
}

pub fn action(value: serde_json::Value) -> UiAction {
    serde_json::from_value(value).unwrap()
}

#[test]
fn config_booleans_and_nesting() {
    for yes in [
        json!(true),
        json!(1),
        json!("Yes"),
        json!(" on "),
        json!("enabled"),
    ] {
        assert_eq!(config::parse_bool(&yes), Some(true), "{yes}");
    }
    for no in [json!(false), json!(0), json!("off"), json!("disabled")] {
        assert_eq!(config::parse_bool(&no), Some(false), "{no}");
    }
    assert_eq!(config::parse_bool(&json!("maybe")), None);
    let partial = config::normalize(
        &json!({ "computer_use": { "headless": "1", "managed_browser": "helium" } }),
    );
    assert_eq!(
        partial,
        Partial {
            headless: Some(true),
            managed_browser: Some(ManagedBrowser::Helium),
            ..Partial::default()
        }
    );
    assert_eq!(
        config::normalize(&json!({ "managed_browser": "firefox" })).managed_browser,
        None
    );
}

#[test]
fn config_merge_order_and_trust() {
    let agent = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(agent.path().join("ext")).unwrap();
    std::fs::create_dir_all(cwd.path().join(".pi/ext")).unwrap();
    std::fs::write(
        config::global_path(agent.path()),
        r#"{"headless": true, "browser_use": false}"#,
    )
    .unwrap();
    std::fs::write(config::project_path(cwd.path()), r#"{"headless": false}"#).unwrap();
    let trusted = config::load(agent.path(), cwd.path(), true, Partial::default());
    assert!(!trusted.config.headless && !trusted.config.browser_use);
    let untrusted = config::load(agent.path(), cwd.path(), false, Partial::default());
    assert!(untrusted.config.headless);
    assert_eq!(untrusted.sources[1].status, SourceStatus::Untrusted);
    let env = config::from_env(|key| (key == "PI_COMPUTER_USE_HEADLESS").then(|| "0".into()));
    assert!(
        !config::load(agent.path(), cwd.path(), false, env)
            .config
            .headless
    );
    let status = config::format_status(&untrusted);
    assert!(
        status.contains("ignored: project is not trusted")
            && status.contains("env overrides: none")
    );
    std::fs::write(config::global_path(agent.path()), "{bad").unwrap();
    assert!(matches!(
        config::load(agent.path(), cwd.path(), false, Partial::default()).sources[0].status,
        SourceStatus::Error(_)
    ));
}

#[test]
fn browser_family_detection() {
    assert!(config::is_browser("Google Chrome", None));
    assert!(config::is_browser("Whatever", Some("com.apple.Safari")));
    assert!(!config::is_browser("Notes", Some("com.apple.Notes")));
    let roots = vec![
        Root {
            app: "Safari".into(),
            ..Root::default()
        },
        Root {
            app: "Notes".into(),
            window_title: "Shopping".into(),
            is_focused: true,
            ..Root::default()
        },
    ];
    let picked = select_roots(roots.clone(), &FindArgs::default(), false);
    assert_eq!(
        picked.iter().map(|r| r.app.as_str()).collect::<Vec<_>>(),
        ["Notes"]
    );
    let by_text = FindArgs {
        text: Some("safari".into()),
        ..FindArgs::default()
    };
    assert_eq!(select_roots(roots, &by_text, true)[0].app, "Safari");
}

#[test]
fn outline_render_search_inspect() {
    assert_eq!(normalize_role("AXTextField"), "text_field");
    assert_eq!(normalize_role("button"), "button");
    let observed = observation("s1");
    let root = &observed.outline.root;
    assert_eq!(root.find("@e3").unwrap().label.as_deref(), Some("Title"));
    let (text, omitted) = root.render(4, 10);
    assert_eq!(
        text,
        "@e1 window \"Notes\"\n  @e2 button \"Save\" [press]\n  @e3 text_field \"Title\" [setValue,focus]"
    );
    assert_eq!(omitted, 0);
    assert_eq!(root.render(4, 1).1, 2);
    let (found, total) = search(root, Some("sav"), None, None, 12);
    assert_eq!(
        (found[0].node.reference.as_str(), found[0].reason, total),
        ("@e2", "prefix", 1)
    );
    assert_eq!(search(root, None, Some("text_field"), None, 12).1, 1);
    assert_eq!(search(root, None, None, Some("press"), 12).1, 1);
    assert!(
        crate::outline::inspect(root.find("@e2").unwrap()).contains("frame: x=0 y=0 w=10 h=10")
    );
}

#[test]
fn store_roots_states_and_outputs() {
    let mut store = Store::default();
    let root = Root {
        pid: 5,
        window_id: 9,
        ..Root::default()
    };
    assert_eq!(store.put_root(root.clone()).window_ref, "@r1");
    assert_eq!(store.put_root(root).window_ref, "@r1");
    assert_eq!(
        store
            .put_root(Root {
                pid: 6,
                ..Root::default()
            })
            .window_ref,
        "@r2"
    );
    for i in 0..=STATE_CAP {
        store.put_state(observation(&format!("s{i}")));
    }
    assert!(store.state("s0").is_none() && store.state(&format!("s{STATE_CAP}")).is_some());
    let reference = store.put_output("hello".into());
    assert_eq!(store.output(&reference), Some("hello"));
    assert_ne!(store.new_state_id(), store.new_state_id());
}

#[test]
fn restore_keeps_newest_state_per_window() {
    let entry = |obs: &Observation| {
        let details = output::details("observe_ui", obs, "", None);
        json!({ "type": "message", "message": { "role": "toolResult", "toolName": "observe_ui", "details": details } })
    };
    let roots = json!({ "type": "message", "message": { "role": "toolResult", "toolName": "find_roots",
        "details": { "tool": "find_roots", "windows": [{ "windowRef": "@r4", "app": "Notes", "pid": 1, "windowId": 2 }] } } });
    let other = json!({ "type": "message", "message": { "role": "toolResult", "toolName": "bash", "details": {} } });
    let mut store = Store::default();
    store.restore_from_branch(&[
        roots,
        entry(&observation("old")),
        other,
        entry(&observation("new")),
    ]);
    assert!(store.state("new").is_some() && store.state("old").is_none());
    assert_eq!(store.roots["@r4"].app, "Notes");
    assert_eq!(
        store
            .put_root(Root {
                pid: 99,
                ..Root::default()
            })
            .window_ref,
        "@r5"
    );
}
