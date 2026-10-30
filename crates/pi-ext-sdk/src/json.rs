//! RFC 7386 JSON merge patch helpers.

use serde_json::{Map, Value};

/// Applies `patch` to `target`: objects merge, `null` removes a key, other values replace.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    let Value::Object(patch) = patch else {
        *target = patch.clone();
        return;
    };
    if !target.is_object() {
        *target = Value::Object(Map::new());
    }
    let Value::Object(target) = target else {
        return;
    };
    for (key, value) in patch {
        if value.is_null() {
            target.remove(key);
        } else {
            merge_patch(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    }
}

/// Folds `next` into `base`. Exact except when `base` deletes a key that `next` sets to an
/// object; use [`merge_patch_diff`] for that case.
pub fn merge_patch_compose(base: &mut Value, next: Value) {
    match (base, next) {
        (Value::Object(base), Value::Object(next)) => {
            for (key, value) in next {
                match base.get_mut(&key) {
                    Some(existing) if existing.is_object() && value.is_object() => {
                        merge_patch_compose(existing, value);
                    }
                    Some(existing) if value.is_object() => *existing = strip_nulls(value),
                    _ => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, next) => *base = next,
    }
}

/// The merge patch that turns `from` into `to`, or `None` when they are equal.
pub fn merge_patch_diff(from: &Value, to: &Value) -> Option<Value> {
    if from == to {
        return None;
    }
    let (Value::Object(from), Value::Object(to)) = (from, to) else {
        return Some(to.clone());
    };
    let mut patch = Map::new();
    for key in from.keys().filter(|key| !to.contains_key(*key)) {
        patch.insert(key.clone(), Value::Null);
    }
    for (key, value) in to {
        let changed = match from.get(key) {
            Some(old) if old.is_object() && value.is_object() => merge_patch_diff(old, value),
            Some(old) if old == value => None,
            _ => Some(value.clone()),
        };
        if let Some(change) = changed {
            patch.insert(key.clone(), change);
        }
    }
    Some(Value::Object(patch))
}

fn strip_nulls(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(_, v)| !v.is_null())
                .map(|(k, v)| (k, strip_nulls(v)))
                .collect(),
        ),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn applies_rfc_examples() {
        let mut doc = json!({ "a": "b", "c": { "d": "e", "f": "g" } });
        merge_patch(&mut doc, &json!({ "a": "z", "c": { "f": null } }));
        assert_eq!(doc, json!({ "a": "z", "c": { "d": "e" } }));
    }

    #[test]
    fn compose_matches_sequential_apply() {
        let doc = json!({ "a": { "x": 1, "y": 2 }, "b": 1 });
        let p1 = json!({ "a": { "x": null }, "b": 7 });
        let p2 = json!({ "a": { "z": 3, "y": null }, "b": { "k": null, "m": 2 } });
        let mut sequential = doc.clone();
        merge_patch(&mut sequential, &p1);
        merge_patch(&mut sequential, &p2);
        let mut composed = p1;
        merge_patch_compose(&mut composed, p2);
        let mut once = doc;
        merge_patch(&mut once, &composed);
        assert_eq!(once, sequential);
    }

    #[test]
    fn diff_round_trips() {
        let from = json!({ "a": { "x": 1, "y": 2 }, "b": 1, "c": [1] });
        let to = json!({ "a": { "z": 3 }, "c": [1], "d": null });
        let patch = merge_patch_diff(&from, &to).unwrap();
        let mut applied = from.clone();
        merge_patch(&mut applied, &patch);
        assert_eq!(applied, json!({ "a": { "z": 3 }, "c": [1] }));
        assert_eq!(merge_patch_diff(&from, &from), None);
    }
}
