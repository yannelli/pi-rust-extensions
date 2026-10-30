use serde_json::{Map, Value, json};

pub const MAX_QUESTIONS: usize = 32;

fn content() -> Vec<Value> {
    vec![
        json!({ "type": "string" }),
        json!({ "type": "array", "items": {} }),
        json!({ "type": "object", "additionalProperties": {} }),
    ]
}

fn entry(description: Option<&str>) -> Value {
    let mut any_of = content();
    any_of.push(json!({ "type": "null" }));
    let mut schema = json!({ "anyOf": any_of });
    if let Some(text) = description {
        schema["description"] = json!(text);
    }
    schema
}

/// The upstream `evaluationSchema` as JSON Schema.
pub fn evaluation_schema() -> Value {
    let instructions = entry(Some(
        "One judgment about the whole state, phrased as a question or a statement.",
    ));
    let noul = json!({
        "type": "object", "additionalProperties": false, "required": ["type"],
        "properties": {
            "type": { "const": "noul", "description": "Yes or no: the probability the instructions hold." },
            "instructions": instructions,
            "criteria": {
                "anyOf": [
                    { "type": "null" },
                    { "type": "object", "additionalProperties": false,
                      "properties": { "true": entry(None), "false": entry(None) } }
                ],
                "description": "Optional: what counts as yes and what counts as no, { true, false }."
            }
        }
    });
    let choice = json!({
        "type": "object", "additionalProperties": false, "required": ["type", "criteria"],
        "properties": {
            "type": { "const": "choice", "description": "Pick one criteria label." },
            "instructions": instructions,
            "criteria": {
                "type": "object", "minProperties": 1, "maxProperties": 64,
                "propertyNames": { "type": "string", "minLength": 1, "maxLength": 200 },
                "additionalProperties": entry(None),
                "description": "The options as a map from label to when it applies: { billing: \"Charges and payments\", other: null }. 1–64 entries."
            }
        }
    });
    let score = json!({
        "type": "object", "additionalProperties": false, "required": ["type", "criteria"],
        "properties": {
            "type": { "const": "score", "description": "Rate against the ordered criteria levels." },
            "instructions": instructions,
            "criteria": {
                "type": "array", "items": entry(None), "minItems": 2, "maxItems": 32,
                "description": "Ordered rubric levels, lowest first: [\"neutral\", \"angry\"]. 2–32 levels."
            }
        }
    });
    json!({
        "type": "object", "additionalProperties": false, "required": ["state", "questions"],
        "properties": {
            "state": { "anyOf": content(), "description": "What to judge: text, or an object whose fields the questions name. Not null." },
            "questions": {
                "type": "object", "minProperties": 1, "maxProperties": MAX_QUESTIONS,
                "propertyNames": { "type": "string", "minLength": 1, "maxLength": 100 },
                "additionalProperties": { "anyOf": [noul, choice, score] },
                "description": "Questions keyed by a short id, as an object map, not an array: { \"urgent\": { type: \"noul\", instructions: ... } }."
            },
            "model": { "type": "string", "minLength": 1, "maxLength": 100, "description": "Jev model id, e.g. jev-latest. Omit for the default." }
        }
    })
}

/// Accept common model near-misses: `options`/`levels`/`choices` for `criteria`, a label list
/// for a Choice, a string for a Noul's `true` criterion.
pub fn normalize(value: Value) -> Value {
    let Value::Object(mut request) = value else {
        return value;
    };
    let Some(Value::Object(questions)) = request.get("questions").cloned() else {
        return Value::Object(request);
    };
    let mut normalized = Map::new();
    for (id, question) in questions {
        let Value::Object(mut item) = question else {
            normalized.insert(id, question);
            continue;
        };
        let aliases = ["options", "levels", "choices"].map(|k| item.remove(k));
        if !item.contains_key("criteria") {
            if let Some(alias) = aliases.into_iter().flatten().next() {
                item.insert("criteria".into(), alias);
            }
        }
        let kind = item.get("type").and_then(Value::as_str).unwrap_or_default();
        let replacement = match (kind, item.get("criteria")) {
            ("choice", Some(Value::Array(labels)))
                if labels
                    .iter()
                    .all(|l| l.as_str().is_some_and(|s| !s.is_empty())) =>
            {
                let map = labels
                    .iter()
                    .filter_map(Value::as_str)
                    .map(|l| (l.to_owned(), Value::Null));
                Some(Value::Object(map.collect()))
            }
            ("noul", Some(Value::String(text))) => Some(json!({ "true": text })),
            _ => None,
        };
        if let Some(criteria) = replacement {
            item.insert("criteria".into(), criteria);
        }
        normalized.insert(id, Value::Object(item));
    }
    request.insert("questions".into(), Value::Object(normalized));
    Value::Object(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_aliases() {
        let request = json!({ "state": "s", "questions": {
            "a": { "type": "choice", "options": ["x", "y"] },
            "b": { "type": "score", "levels": ["low", "high"] },
            "c": { "type": "noul", "criteria": "it holds" },
            "d": { "type": "choice", "criteria": { "k": null }, "choices": ["z"] },
            "e": 3
        }});
        let out = normalize(request);
        let q = &out["questions"];
        assert_eq!(q["a"]["criteria"], json!({ "x": null, "y": null }));
        assert_eq!(q["b"]["criteria"], json!(["low", "high"]));
        assert_eq!(q["c"]["criteria"], json!({ "true": "it holds" }));
        assert_eq!(
            q["d"],
            json!({ "type": "choice", "criteria": { "k": null } })
        );
        assert_eq!(q["e"], json!(3));
        assert_eq!(normalize(json!([1])), json!([1]));
    }

    #[test]
    fn schema_matches_upstream_bounds() {
        let schema = evaluation_schema();
        assert_eq!(schema["additionalProperties"], json!(false));
        assert_eq!(
            schema["properties"]["questions"]["maxProperties"],
            json!(32)
        );
        let variants = &schema["properties"]["questions"]["additionalProperties"]["anyOf"];
        assert_eq!(
            variants[1]["properties"]["criteria"]["maxProperties"],
            json!(64)
        );
        assert_eq!(variants[2]["properties"]["criteria"]["minItems"], json!(2));
    }
}
