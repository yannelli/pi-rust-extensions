use serde::Deserialize;
use serde_json::{Value, json};

pub type Props = Vec<(&'static str, Value)>;

const SCRIPT: &str = include_str!("snapshot.js");
pub const MAX_ELEMENTS: u64 = 80;
pub const MODES: &[&str] = &[
    "auto",
    "interactive",
    "forms",
    "pageMap",
    "text",
    "changes",
    "full",
];

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapshotArgs {
    pub mode: Option<String>,
    pub max_elements: Option<u64>,
    pub query: Option<String>,
    pub max_text_chars: Option<u64>,
    pub containing_text: Option<String>,
    pub role_filter: Option<String>,
    pub near_uid: Option<String>,
}

impl SnapshotArgs {
    pub fn mode(&self) -> &str {
        self.mode.as_deref().unwrap_or("auto")
    }

    pub fn expression(&self) -> String {
        let mode = self.mode();
        let default_text = match mode {
            "auto" => 1_500,
            "text" => 8_000,
            "full" => 20_000,
            _ => 0,
        };
        let max_elements = match mode {
            "text" => 0,
            _ => self.max_elements.unwrap_or(MAX_ELEMENTS),
        };
        let opts = json!({
            "mode": mode,
            "maxElements": max_elements,
            "maxTextChars": self.max_text_chars.unwrap_or(default_text),
            "query": self.query,
            "containingText": self.containing_text,
            "roleFilter": self.role_filter,
            "nearUid": self.near_uid,
        });
        format!("{}({opts})", SCRIPT.trim_end())
    }
}

/// One line per element: `[e3] button "Sign in" (120,48)`.
pub fn element_lines(snapshot: &Value) -> Vec<String> {
    let empty = Vec::new();
    let elements = snapshot["elements"].as_array().unwrap_or(&empty);
    elements.iter().map(element_line).collect()
}

fn element_line(element: &Value) -> String {
    let field = |key: &str| element.get(key).and_then(Value::as_str).unwrap_or("");
    let mut line = format!("[{}] {}", field("uid"), field("role"));
    if !field("name").is_empty() {
        line.push_str(&format!(" {:?}", field("name")));
    }
    if element.get("value").is_some() {
        line.push_str(&format!(" value={:?}", field("value")));
    }
    for flag in ["disabled", "checked", "offscreen"] {
        if element.get(flag) == Some(&Value::Bool(true)) {
            line.push_str(&format!(" {flag}"));
        }
    }
    let coord = |key: &str| element.get(key).and_then(Value::as_i64).unwrap_or(0);
    line.push_str(&format!(" ({},{})", coord("x"), coord("y")));
    line
}

pub fn diff(previous: &[String], current: &[String]) -> (Vec<String>, Vec<String>) {
    let added = current
        .iter()
        .filter(|l| !previous.contains(l))
        .cloned()
        .collect();
    let removed = previous
        .iter()
        .filter(|l| !current.contains(l))
        .cloned()
        .collect();
    (added, removed)
}

pub fn format(snapshot: &Value, mode: &str, previous: Option<&[String]>) -> String {
    let title = snapshot["title"].as_str().unwrap_or("");
    let url = snapshot["url"].as_str().unwrap_or("");
    let mut out = vec![format!("Page: {title}"), format!("URL: {url}")];
    if let Some(scroll) = snapshot.get("scroll") {
        out.push(format!(
            "Scroll: {} of {} px (viewport {} px)",
            scroll["y"], scroll["height"], scroll["viewport"]
        ));
    }
    let lines = element_lines(snapshot);
    let total = snapshot["total"].as_u64().unwrap_or(lines.len() as u64);
    if mode == "changes" {
        match previous {
            None => out.push("No earlier snapshot of this tab. Showing all elements.".to_owned()),
            Some(previous) => {
                let (added, removed) = diff(previous, &lines);
                out.push(format!(
                    "Changes: {} added, {} removed",
                    added.len(),
                    removed.len()
                ));
                out.extend(added.into_iter().map(|l| format!("+ {l}")));
                out.extend(removed.into_iter().map(|l| format!("- {l}")));
                return out.join("\n");
            }
        }
    }
    if mode != "text" {
        out.push(format!("Elements ({} of {total}):", lines.len()));
        out.extend(lines);
    }
    let text = snapshot["text"].as_str().unwrap_or("").trim();
    if !text.is_empty() {
        out.push("Text:".to_owned());
        out.push(text.to_owned());
    }
    out.join("\n")
}

pub fn snapshot_props() -> Props {
    vec![
        (
            "mode",
            json!({ "type": "string", "enum": MODES, "description": "auto (default), interactive, forms, pageMap, text, changes, or full." }),
        ),
        (
            "maxElements",
            json!({ "type": "number", "default": MAX_ELEMENTS }),
        ),
        (
            "query",
            crate::specs::s("Rank elements that match this phrase, for example 'sign in button'."),
        ),
        (
            "maxTextChars",
            crate::specs::n("Maximum characters of page text."),
        ),
        (
            "containingText",
            crate::specs::s("Only elements whose label or value contains this text."),
        ),
        (
            "roleFilter",
            crate::specs::s("Only elements with this role or tag, for example button or textbox."),
        ),
        (
            "nearUid",
            crate::specs::s("Sort elements by distance from this uid."),
        ),
    ]
}

pub fn include_snapshot() -> Props {
    vec![
        (
            "includeSnapshot",
            crate::specs::b("Add a new snapshot after the action."),
        ),
        (
            "maxElements",
            json!({ "type": "number", "default": MAX_ELEMENTS, "description": "Maximum elements in that snapshot." }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        json!({
            "url": "https://a.dev", "title": "A", "total": 3,
            "scroll": { "y": 0, "height": 900, "viewport": 600 },
            "elements": [
                { "uid": "e1", "role": "button", "name": "Sign in", "x": 10, "y": 20 },
                { "uid": "e2", "role": "textbox", "name": "Email", "value": "a@b", "x": 5, "y": 6, "disabled": true }
            ],
            "text": "Hello"
        })
    }

    #[test]
    fn formats_elements_and_text() {
        let text = format(&sample(), "auto", None);
        assert!(text.contains("Elements (2 of 3):"));
        assert!(text.contains(r#"[e1] button "Sign in" (10,20)"#));
        assert!(text.contains(r#"[e2] textbox "Email" value="a@b" disabled (5,6)"#));
        assert!(text.ends_with("Text:\nHello"));
    }

    #[test]
    fn changes_mode_diffs_lines() {
        let previous = vec![
            "[e9] link \"Old\" (1,1)".to_owned(),
            element_lines(&sample())[0].clone(),
        ];
        let text = format(&sample(), "changes", Some(&previous));
        assert!(text.contains("Changes: 1 added, 1 removed"));
        assert!(text.contains("+ [e2] textbox"));
        assert!(text.contains("- [e9] link"));
    }

    #[test]
    fn expression_applies_mode_defaults() {
        let args = SnapshotArgs {
            mode: Some("text".to_owned()),
            ..SnapshotArgs::default()
        };
        let expr = args.expression();
        assert!(expr.starts_with("((opts) =>"));
        assert!(expr.contains(r#""maxElements":0"#) && expr.contains(r#""maxTextChars":8000"#));
        let auto = SnapshotArgs::default().expression();
        assert!(auto.contains(r#""maxElements":80"#) && auto.contains(r#""maxTextChars":1500"#));
    }
}
