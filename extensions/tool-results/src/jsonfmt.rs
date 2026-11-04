//! JSON results: detection, stripping, and the TOON, compact, and table forms.

use serde_json::{Map, Value};

use crate::blobs;
use crate::config::{JsonFormat, Settings};
use crate::toon;

/// The JSON value when the whole block is one JSON object or array, bare or in a code fence.
pub fn parse(text: &str) -> Option<Value> {
    let mut body = text.trim();
    if let Some(rest) = body.strip_prefix("```") {
        let rest = rest.strip_prefix("json").unwrap_or(rest);
        body = rest.strip_suffix("```")?.trim();
    }
    if !(body.starts_with('{') || body.starts_with('[')) {
        return None;
    }
    serde_json::from_str(body).ok()
}

/// Drops null, empty strings, empty arrays, and empty objects from objects, and `false` and
/// `0` too with `defaults`. Array items stay in place.
pub fn strip(value: Value, defaults: bool) -> Value {
    match value {
        Value::Object(map) => {
            let kept: Map<String, Value> = map
                .into_iter()
                .map(|(k, v)| (k, strip(v, defaults)))
                .filter(|(_, v)| !is_empty(v, defaults))
                .collect();
            Value::Object(kept)
        }
        Value::Array(items) => {
            Value::Array(items.into_iter().map(|v| strip(v, defaults)).collect())
        }
        other => other,
    }
}

fn is_empty(value: &Value, defaults: bool) -> bool {
    match value {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        Value::Array(a) => a.is_empty(),
        Value::Object(m) => m.is_empty(),
        Value::Bool(b) => defaults && !b,
        Value::Number(n) => defaults && n.as_f64() == Some(0.0),
    }
}

fn elide_strings(value: &mut Value, min: usize) -> bool {
    match value {
        Value::String(s) => match blobs::elide(s, min) {
            Some(out) => {
                *s = out;
                true
            }
            None => false,
        },
        Value::Array(items) => items
            .iter_mut()
            .fold(false, |acc, v| elide_strings(v, min) | acc),
        Value::Object(map) => map
            .values_mut()
            .fold(false, |acc, v| elide_strings(v, min) | acc),
        _ => false,
    }
}

/// The keys of a non-empty array of objects that all share one key set.
fn uniform(items: &[Value]) -> Option<Vec<String>> {
    let first = items.first()?.as_object()?;
    let same = items.iter().all(|item| {
        item.as_object()
            .is_some_and(|m| m.len() == first.len() && first.keys().all(|k| m.contains_key(k)))
    });
    (same && !first.is_empty()).then(|| first.keys().cloned().collect())
}

fn cell(value: &Value) -> String {
    let text = match value {
        Value::Null => return String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let quote = text.contains([',', '"', '\n', '\r']) || text.trim() != text;
    if quote {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text
    }
}

/// RFC 4180 CSV with a header row; nested values become compact JSON cells.
pub fn csv(items: &[Value]) -> Option<String> {
    let keys = uniform(items)?;
    let mut lines = vec![
        keys.iter()
            .map(|k| cell(&Value::String(k.clone())))
            .collect::<Vec<_>>()
            .join(","),
    ];
    for item in items {
        let row: Vec<String> = keys.iter().map(|k| cell(&item[k.as_str()])).collect();
        lines.push(row.join(","));
    }
    Some(lines.join("\n"))
}

/// A root array as CSV, or a root object with its uniform arrays as CSV blocks and the other
/// fields as `key: value` lines. `None` when nothing is tabular.
pub fn table(value: &Value) -> Option<String> {
    match value {
        Value::Array(items) => csv(items),
        Value::Object(map) => {
            let mut found = false;
            let mut out = Vec::new();
            for (key, value) in map {
                match value {
                    Value::Array(items) if uniform(items).is_some() => {
                        found = true;
                        out.push(format!("{key} ({} rows):", items.len()));
                        out.push(csv(items)?);
                    }
                    Value::String(s) => out.push(format!("{key}: {s}")),
                    other => out.push(format!("{key}: {other}")),
                }
            }
            found.then(|| out.join("\n"))
        }
        _ => None,
    }
}

/// The converted block when the text is JSON and the result is shorter; `None` otherwise.
pub fn convert(text: &str, settings: &Settings) -> Option<String> {
    if settings.json == JsonFormat::Off {
        return None;
    }
    let mut value = parse(text)?;
    if settings.base64 {
        elide_strings(&mut value, settings.base64_min);
    }
    if settings.json_strip || settings.json == JsonFormat::Stripped {
        value = strip(value, settings.json_drop_defaults);
    }
    let compact = || serde_json::to_string(&value).unwrap_or_default();
    let out = match settings.json {
        JsonFormat::Toon => toon::encode(&value),
        JsonFormat::Table => table(&value).unwrap_or_else(compact),
        JsonFormat::Compact | JsonFormat::Stripped | JsonFormat::Off => compact(),
    };
    (out.chars().count() < text.chars().count()).then_some(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn with(format: JsonFormat) -> Settings {
        Settings {
            json: format,
            ..Settings::default()
        }
    }

    #[test]
    fn detects_whole_block_json_only() {
        assert_eq!(parse(" {\"a\": 1}\n"), Some(json!({ "a": 1 })));
        assert_eq!(parse("```json\n[1, 2]\n```"), Some(json!([1, 2])));
        assert_eq!(parse("result: {\"a\": 1}"), None);
        assert_eq!(parse("42"), None);
        assert_eq!(parse("{broken"), None);
    }

    #[test]
    fn strips_null_and_empty_fields() {
        let value = json!({ "a": null, "b": "", "c": [], "d": {}, "e": { "f": null }, "g": false, "h": 0, "i": [null, 1] });
        assert_eq!(
            strip(value.clone(), false),
            json!({ "g": false, "h": 0, "i": [null, 1] })
        );
        assert_eq!(strip(value, true), json!({ "i": [null, 1] }));
    }

    #[test]
    fn converts_pretty_json() {
        let text = serde_json::to_string_pretty(&json!({ "users": [
            { "id": 1, "name": "Ada", "email": null },
            { "id": 2, "name": "Bob", "email": null }
        ]}))
        .unwrap();
        assert_eq!(
            convert(&text, &with(JsonFormat::Toon)).unwrap(),
            "users[2]{id,name,email}:\n  1,Ada,null\n  2,Bob,null"
        );
        assert_eq!(
            convert(&text, &with(JsonFormat::Compact)).unwrap(),
            r#"{"users":[{"id":1,"name":"Ada","email":null},{"id":2,"name":"Bob","email":null}]}"#
        );
        assert_eq!(
            convert(&text, &with(JsonFormat::Stripped)).unwrap(),
            r#"{"users":[{"id":1,"name":"Ada"},{"id":2,"name":"Bob"}]}"#
        );
        assert_eq!(
            convert(&text, &with(JsonFormat::Table)).unwrap(),
            "users (2 rows):\nid,name,email\n1,Ada,\n2,Bob,"
        );
        assert_eq!(convert(&text, &with(JsonFormat::Off)), None);
    }

    #[test]
    fn keeps_the_original_when_not_smaller() {
        let text = r#"{"a":1}"#;
        assert_eq!(convert(text, &with(JsonFormat::Compact)), None);
        assert_eq!(convert(text, &with(JsonFormat::Toon)), Some("a: 1".into()));
        assert_eq!(convert("[[1,2],[3,4]]", &with(JsonFormat::Toon)), None);
    }

    #[test]
    fn csv_quotes_cells() {
        let rows = json!([{ "a": "x,y", "b": "say \"hi\"" }, { "a": " pad", "b": [1, 2] }]);
        assert_eq!(
            csv(rows.as_array().unwrap()).unwrap(),
            "a,b\n\"x,y\",\"say \"\"hi\"\"\"\n\" pad\",\"[1,2]\""
        );
        assert_eq!(
            csv(json!([{ "a": 1 }, { "b": 2 }]).as_array().unwrap()),
            None
        );
    }
}
