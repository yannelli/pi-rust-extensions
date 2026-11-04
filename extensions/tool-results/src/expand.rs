//! The `details` record of a changed result and the lines of its expanded view.

use pi_ext_sdk::style;
use serde_json::{Map, Value, json};

use crate::lines::short;
use crate::store::Meta;

/// The key in `details` that the host's expander looks for.
pub const DETAILS_KEY: &str = "toolResults";

/// `details.toolResults`: what was stored, so a resumed session knows it, and the full text
/// with `embed`.
pub fn record(meta: &Meta, full: &str, embed: bool) -> Value {
    let mut record = json!({
        "resultId": meta.id,
        "chars": meta.chars,
        "lines": meta.lines,
        "hash": meta.hash,
    });
    if embed {
        record["original"] = json!(full);
    }
    record
}

/// The merge patch for `details`; `None` when the tool's `details` is not an object or null.
pub fn details_patch(details: Option<&Value>, record: Value) -> Option<Value> {
    match details {
        None | Some(Value::Null) | Some(Value::Object(_)) => {
            let mut patch = Map::new();
            patch.insert(DETAILS_KEY.into(), record);
            Some(Value::Object(patch))
        }
        Some(_) => None,
    }
}

/// Splits `text` into lines of at most `width` columns; tabs become 4 spaces.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(8);
    let mut out = Vec::new();
    for line in text.split('\n') {
        let line = line.replace('\t', "    ");
        let mut current = String::new();
        let mut used = 0;
        for c in line.chars() {
            let w = style::char_width(c);
            if used + w > width {
                out.push(std::mem::take(&mut current));
                used = 0;
            }
            current.push(c);
            used += w;
        }
        out.push(current);
    }
    out
}

/// The expanded view: a header line, then the full text wrapped to `width`.
pub fn view(id: &str, text: Option<&str>, width: usize) -> Vec<String> {
    let Some(text) = text else {
        let line =
            format!("The full result {id} is no longer stored (retention or a purge removed it).");
        return vec![style::dim(&line)];
    };
    let header = format!(
        "Full result {id} · {} characters · not sent to the model",
        short(text.chars().count())
    );
    let mut lines = vec![style::dim(&style::truncate(&header, width))];
    lines.extend(wrap(text, width));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_long_lines_and_tabs() {
        assert_eq!(wrap("abcdefghij\n\tx", 8), ["abcdefgh", "ij", "    x"]);
        assert_eq!(wrap("界界界界界", 8), ["界界界界", "界"]);
        assert_eq!(wrap("", 20), [""]);
    }

    #[test]
    fn view_has_a_header_or_says_the_result_is_gone() {
        let lines = view("tr-1", Some("a\nb"), 80);
        assert_eq!(
            style::strip(&lines[0]),
            "Full result tr-1 · 3 characters · not sent to the model"
        );
        assert_eq!(lines[1..], ["a", "b"]);
        let gone = view("tr-2", None, 80);
        assert!(style::strip(&gone[0]).contains("tr-2 is no longer stored"));
    }

    #[test]
    fn records_go_into_object_or_missing_details_only() {
        let meta = Meta {
            id: "tr-1".into(),
            chars: 5,
            lines: 1,
            hash: "sha256:x".into(),
            ..Meta::default()
        };
        let plain = record(&meta, "hello", false);
        assert_eq!(
            plain,
            json!({ "resultId": "tr-1", "chars": 5, "lines": 1, "hash": "sha256:x" })
        );
        assert_eq!(record(&meta, "hello", true)["original"], "hello");
        let patch = details_patch(Some(&json!({ "exitCode": 0 })), plain.clone()).unwrap();
        assert_eq!(patch, json!({ "toolResults": plain }));
        assert!(details_patch(None, plain.clone()).is_some());
        assert!(details_patch(Some(&json!("text")), plain).is_none());
    }
}
