use serde_json::Value;

fn probability(value: &Value) -> bool {
    value.as_f64().is_some_and(|p| (0.0..=1.0).contains(&p))
}

fn token_count(value: &Value) -> bool {
    value.is_null() || value.as_u64().is_some()
}

/// The upstream `validResult`: one well-formed answer per question, probabilities in [0, 1].
pub fn valid_result(result: &Value, questions: &Value) -> bool {
    let (Some(questions), Some(answers)) = (questions.as_object(), result["answers"].as_object())
    else {
        return false;
    };
    let model_ok = result["model"].as_str().is_some_and(|m| !m.is_empty());
    let usage = &result["usage"];
    if !model_ok
        || !usage.is_object()
        || !token_count(&usage["input_tokens"])
        || !token_count(&usage["output_tokens"])
    {
        return false;
    }
    if answers.len() != questions.len() {
        return false;
    }
    questions.iter().all(|(id, question)| {
        let Some(answer) = answers.get(id) else {
            return false;
        };
        let kind = question["type"].as_str().unwrap_or_default();
        if answer["type"].as_str() != Some(kind) {
            return false;
        }
        if kind == "noul" {
            return probability(&answer["noul"]);
        }
        let keys: Vec<String> = match kind {
            "choice" => question["criteria"]
                .as_object()
                .map(|c| c.keys().cloned().collect()),
            "score" => question["criteria"]
                .as_array()
                .map(|l| (0..l.len()).map(|i| i.to_string()).collect()),
            _ => None,
        }
        .unwrap_or_default();
        let Some(probabilities) = answer["probabilities"].as_object() else {
            return false;
        };
        if !probability(&answer["confidence"]) || probabilities.len() != keys.len() {
            return false;
        }
        if !keys
            .iter()
            .all(|k| probabilities.get(k).is_some_and(probability))
        {
            return false;
        }
        match kind {
            "choice" => answer["choice"]
                .as_str()
                .is_some_and(|c| keys.iter().any(|k| k == c)),
            _ => {
                let max = keys.len().saturating_sub(1) as f64;
                let in_range = answer["score"]
                    .as_f64()
                    .is_some_and(|s| (0.0..=max).contains(&s));
                in_range && answer.get("legend").is_some()
            }
        }
    })
}

fn fixed(value: &Value) -> String {
    value
        .as_f64()
        .map_or_else(|| "not reported".to_owned(), |v| format!("{v:.3}"))
}

fn count(value: &Value) -> String {
    value
        .as_u64()
        .map_or_else(|| "not reported".to_owned(), |v| v.to_string())
}

/// The upstream result text: one line per answer, tokens, and the disclaimer.
pub fn format_result(result: &Value, expanded: bool) -> String {
    let mut lines = vec![format!(
        "TypeSafe · {} · {} ms",
        result["model"],
        result["elapsedMs"].as_u64().unwrap_or(0)
    )];
    for (id, answer) in result["answers"].as_object().into_iter().flatten() {
        let label = Value::String(id.clone());
        let kind = answer["type"].as_str().unwrap_or_default();
        lines.push(match kind {
            "noul" => format!("{label}: P(yes) = {}", fixed(&answer["noul"])),
            "choice" => format!(
                "{label}: {} · confidence {}",
                answer["choice"],
                fixed(&answer["confidence"])
            ),
            "score" => format!(
                "{label}: {} · confidence {}",
                fixed(&answer["score"]),
                fixed(&answer["confidence"])
            ),
            _ => format!(
                "{label}: box {} · confidence {}",
                answer["box"],
                fixed(&answer["confidence"])
            ),
        });
        if expanded && matches!(kind, "choice" | "score") {
            lines.push(format!("  {}", answer["probabilities"]));
        }
    }
    let usage = &result["usage"];
    lines.push(format!(
        "{} input / {} output tokens",
        count(&usage["input_tokens"]),
        count(&usage["output_tokens"])
    ));
    lines.push("Confidence is distribution concentration, not proof of correctness.".to_owned());
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn questions() -> Value {
        json!({
            "c": { "type": "choice", "criteria": { "a": null, "b": "x" } },
            "s": { "type": "score", "criteria": ["lo", "hi"] },
            "n": { "type": "noul" }
        })
    }

    pub fn answer() -> Value {
        json!({
            "model": "jev-1.13", "usage": { "input_tokens": 12, "output_tokens": null },
            "answers": {
                "c": { "type": "choice", "choice": "b", "confidence": 0.9, "probabilities": { "a": 0.1, "b": 0.9 } },
                "s": { "type": "score", "score": 0.25, "confidence": 0.5, "legend": {}, "probabilities": { "0": 0.75, "1": 0.25 } },
                "n": { "type": "noul", "noul": 0.123 }
            }
        })
    }

    #[test]
    fn validates_answers() {
        assert!(valid_result(&answer(), &questions()));
        let mut bad = answer();
        bad["answers"]["c"]["choice"] = json!("z");
        assert!(!valid_result(&bad, &questions()));
        let mut bad = answer();
        bad["answers"]["s"]["score"] = json!(1.5);
        assert!(!valid_result(&bad, &questions()));
        let mut bad = answer();
        bad["answers"]["n"]["noul"] = json!(1.2);
        assert!(!valid_result(&bad, &questions()));
        let mut bad = answer();
        bad["usage"]["input_tokens"] = json!(-1);
        assert!(!valid_result(&bad, &questions()));
        let mut bad = answer();
        bad["answers"].as_object_mut().unwrap().remove("n");
        assert!(!valid_result(&bad, &questions()));
    }

    #[test]
    fn formats_like_upstream() {
        let mut result = answer();
        result["elapsedMs"] = json!(42);
        let text = format_result(&result, true);
        assert!(text.starts_with("TypeSafe · \"jev-1.13\" · 42 ms"));
        assert!(text.contains("\"c\": \"b\" · confidence 0.900"));
        assert!(text.contains("\"n\": P(yes) = 0.123"));
        assert!(text.contains("  {\"0\":0.75,\"1\":0.25}"));
        assert!(text.contains("12 input / not reported output tokens"));
        assert!(!format_result(&result, false).contains("{\"a\""));
    }
}
