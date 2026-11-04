use pi_ext_compact_tui::Config;
use pi_ext_sdk::PromptLayout;
use serde_json::json;

#[test]
fn editor_defaults_and_config_keys() {
    let defaults: Config = serde_json::from_value(json!({})).unwrap();
    assert_eq!(defaults, Config::default());
    assert_eq!(defaults.border_color, "thinking");
    assert_eq!(defaults.prompt_layout, PromptLayout::Frame);
    for (name, layout) in [
        ("frame", PromptLayout::Frame),
        ("caret", PromptLayout::Caret),
        ("harbor", PromptLayout::Harbor),
    ] {
        let config: Config = serde_json::from_value(json!({
            "borderColor": "#12abEF", "promptLayout": name, "editor": false
        }))
        .unwrap();
        assert_eq!(config.border_color, "#12abEF");
        assert_eq!(config.prompt_layout, layout);
        assert!(!config.editor);
    }
}

#[test]
fn invalid_editor_config_preserves_other_options() {
    for value in [json!(false), json!(null), json!(123), json!([]), json!({})] {
        let config: Config = serde_json::from_value(json!({
            "borderColor": value, "promptLayout": value, "summary": false
        }))
        .unwrap();
        assert_eq!(config.border_color, "thinking");
        assert_eq!(config.prompt_layout, PromptLayout::Frame);
        assert!(!config.summary);
    }
    let config: Config = serde_json::from_value(json!({ "promptLayout": "unknown" })).unwrap();
    assert_eq!(config.prompt_layout, PromptLayout::Frame);
}
