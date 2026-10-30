use serde_json::json;

use super::logic::{action, observation};
use crate::act::Planner;
use crate::cond::{Condition, diff};
use crate::output;
use crate::platform::Step;
use crate::state::Store;

#[test]
fn output_bounds_and_pages() {
    let mut store = Store::default();
    let long = "line\n".repeat(output::MAX_LINES + 10);
    let bounded = output::bound(&mut store, long.clone());
    assert!(bounded.contains("read_text({ ref: \"@o1\""));
    assert_eq!(store.output("@o1"), Some(long.as_str()));
    assert_eq!(output::bound(&mut store, "short".into()), "short");
    assert!(output::output_page("héllo", 2).starts_with("éllo"));
    assert!(output::text_page(&"x".repeat(5000), 0).contains("Continue with offset 4000"));
}

#[test]
fn conditions_validate_and_hold() {
    let bad = |v| {
        serde_json::from_value::<Condition>(v)
            .unwrap()
            .validate()
            .unwrap_err()
    };
    assert_eq!(
        bad(json!({ "ref": "@e1", "scopeRef": "@e1", "text": "x" })),
        "A UI condition accepts ref or scopeRef, not both."
    );
    assert_eq!(
        bad(json!({})),
        "A UI condition requires text, role, or value."
    );
    assert_eq!(
        bad(json!({ "role": "button" })),
        "A role-only UI condition requires ref or scopeRef."
    );
    assert_eq!(
        bad(json!({ "value": "x" })),
        "A value UI condition requires an exact ref."
    );
    let base = observation("s1").outline.root;
    let mut after = base.clone();
    after.children[1].value = Some("Done".into());
    let holds = |v| {
        serde_json::from_value::<Condition>(v)
            .unwrap()
            .holds(&base, &after)
            .unwrap()
    };
    assert!(holds(json!({ "ref": "@e3", "value": "Done" })));
    assert!(!holds(json!({ "text": "Cancel" })));
    assert!(holds(json!({ "text": "Cancel", "until": "absent" })));
    assert!(holds(json!({ "scopeRef": "@e1", "role": "button" })));
    let changes = diff(&base, &after);
    assert_eq!((changes.added.len(), changes.removed), (1, 1));
}

#[test]
fn planner_maps_actions_to_steps() {
    let observed = observation("s1");
    let planner = |headless, semantic_tree| Planner {
        observation: &observed,
        semantic_tree,
        headless,
        mac: true,
    };
    let press = [action(json!({ "action": "press", "ref": "@e2" }))];
    assert_eq!(
        planner(false, true).plan(&press).unwrap(),
        [Step::Press { path: vec![0] }]
    );
    assert_eq!(
        planner(false, false).plan(&press).unwrap(),
        [
            Step::Activate,
            Step::Click {
                x: 5.0,
                y: 5.0,
                button: "left".into(),
                count: 1
            }
        ]
    );
    let set = [action(
        json!({ "action": "setText", "ref": "@e3", "text": "Hi" }),
    )];
    assert_eq!(
        planner(true, true).plan(&set).unwrap(),
        [Step::SetText {
            path: vec![1],
            text: "Hi".into()
        }]
    );
    let click = [action(
        json!({ "action": "click", "x": 400, "y": 300, "clickCount": 5 }),
    )];
    assert_eq!(
        planner(false, true).plan(&click).unwrap(),
        [
            Step::Activate,
            Step::Click {
                x: 300.0,
                y: 250.0,
                button: "left".into(),
                count: 3
            }
        ]
    );
    assert!(
        planner(true, true)
            .plan(&click)
            .unwrap_err()
            .starts_with("headless is enabled")
    );
    let outside = [action(json!({ "action": "moveMouse", "x": 900, "y": 1 }))];
    assert!(
        planner(false, true)
            .plan(&outside)
            .unwrap_err()
            .contains("outside the latest look image bounds (800x600)")
    );
    let stale = [action(json!({ "action": "press", "ref": "@e9" }))];
    assert!(
        planner(false, true)
            .plan(&stale)
            .unwrap_err()
            .starts_with("Outline ref '@e9' is stale")
    );
    let typing = [
        action(json!({ "action": "typeText", "ref": "@e3", "text": "x" })),
        action(json!({ "action": "keypress", "keys": ["cmd+s"] })),
    ];
    assert_eq!(
        planner(false, true).plan(&typing).unwrap(),
        [
            Step::Activate,
            Step::Focus { path: vec![1] },
            Step::Type { text: "x".into() },
            Step::Keys {
                keys: vec!["cmd".into(), "s".into()]
            }
        ]
    );
    assert!(planner(false, true).plan(&[]).is_err());
}
