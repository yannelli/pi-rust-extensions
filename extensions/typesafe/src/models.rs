//! Model listings with context, price, and expiry, and the picker lines `/typesafe models` shows.

use serde_json::Value;

use crate::backends::Backend;
use crate::errors::{Code, TsError, TsResult};

#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub name: Option<String>,
    pub context_length: Option<u64>,
    pub usd_per_mtok_input: Option<f64>,
    pub expires: Option<String>,
}

impl ModelInfo {
    /// `typesafe/jev-1.14 · 262K ctx · $0.042/M in · expires 2026-11-20`, with `* ` for the current model.
    pub fn line(&self, current: bool) -> String {
        let mut parts = vec![self.id.clone()];
        if let Some(context) = self.context_length {
            parts.push(format!("{} ctx", tokens(context)));
        }
        match self.usd_per_mtok_input {
            Some(0.0) => parts.push("free".to_owned()),
            Some(price) => parts.push(format!("${}/M in", usd(price))),
            None => {}
        }
        if let Some(expires) = &self.expires {
            parts.push(format!("expires {expires}"));
        }
        let marker = if current { "* " } else { "" };
        format!("{marker}{}", parts.join(" · "))
    }
}

fn tokens(count: u64) -> String {
    match count {
        1_000_000.. => format!("{}M", trim(format!("{:.1}", count as f64 / 1e6))),
        1_000.. => format!("{}K", (count as f64 / 1e3).round()),
        _ => count.to_string(),
    }
}

fn usd(price: f64) -> String {
    trim(format!("{price:.6}"))
}

fn trim(number: String) -> String {
    match number.contains('.') {
        true => number
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_owned(),
        false => number,
    }
}

/// The models of a listing response, aliases (`~`) first, then by id.
pub fn parse_models(backend: &Backend, wire: &Value) -> TsResult<Vec<ModelInfo>> {
    let list = wire
        .get(backend.models_field)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            TsError::new(
                Code::Response,
                "TypeSafe returned an unexpected model list.",
            )
        })?;
    let mut models: Vec<ModelInfo> = list
        .iter()
        .filter(|entry| !backend.models_by_endpoint || serves(entry, backend.path))
        .filter_map(|entry| model(backend, entry))
        .collect();
    models.sort_by(|a, b| {
        let alias = |m: &ModelInfo| !m.id.starts_with('~');
        alias(a).cmp(&alias(b)).then_with(|| a.id.cmp(&b.id))
    });
    Ok(models)
}

fn serves(entry: &Value, path: &str) -> bool {
    entry
        .get("endpoints")
        .and_then(Value::as_array)
        .is_some_and(|endpoints| endpoints.iter().any(|e| e.as_str() == Some(path)))
}

fn model(backend: &Backend, entry: &Value) -> Option<ModelInfo> {
    let text = |field: &str| {
        entry
            .get(field)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
    };
    let id = entry
        .get(backend.models_id_field)
        .or_else(|| entry.get("name"))
        .and_then(Value::as_str)
        .filter(|n| (1..=100).contains(&n.len()))?
        .to_owned();
    let per_token = entry["pricing"]["prompt"]
        .as_str()
        .and_then(|p| p.parse::<f64>().ok())
        .filter(|p| p.is_finite() && *p >= 0.0);
    let expires = text("expiration_date").map(|d| d.split('T').next().unwrap_or(&d).to_owned());
    Some(ModelInfo {
        name: text("name").filter(|name| *name != id),
        context_length: entry["context_length"].as_u64().filter(|n| *n > 0),
        usd_per_mtok_input: per_token.map(|p| p * 1e6).or_else(|| backend.price(&id)),
        expires,
        id,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::backends::backend;

    fn openrouter_listing() -> Value {
        json!({ "data": [
            { "id": "typesafe/jev-1.14", "name": "TypeSafe: Jev 1.14", "context_length": 262144,
              "pricing": { "prompt": "0.000000042", "completion": "0" },
              "endpoints": ["/api/alpha/decisions", "/api/alpha/extractions"] },
            { "id": "~typesafe/jev-latest", "name": "Jev latest", "context_length": 0,
              "pricing": { "prompt": "0" }, "endpoints": ["/api/alpha/decisions"],
              "alias_target": { "name": "Jev 1.14", "slug": "typesafe/jev-1.14" } },
            { "id": "typesafe/jev-1.13", "context_length": 1048576, "pricing": { "prompt": "0.00000005" },
              "endpoints": ["/api/alpha/decisions"], "expiration_date": "2026-11-20T00:00:00Z" },
            { "id": "vendor/extract", "pricing": { "prompt": "0.000001" }, "endpoints": ["/api/alpha/extractions"] },
            { "id": "openai/gpt-x", "context_length": 128000, "pricing": { "prompt": "0.000002" } }
        ]})
    }

    #[test]
    fn parses_openrouter_decision_listing() {
        let models = parse_models(&backend("openrouter").unwrap(), &openrouter_listing()).unwrap();
        let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "~typesafe/jev-latest",
                "typesafe/jev-1.13",
                "typesafe/jev-1.14"
            ]
        );
        let jev = &models[2];
        assert_eq!(jev.name.as_deref(), Some("TypeSafe: Jev 1.14"));
        assert_eq!(jev.context_length, Some(262144));
        assert!((jev.usd_per_mtok_input.unwrap() - 0.042).abs() < 1e-12);
        assert_eq!(models[0].context_length, None);
        assert_eq!(models[1].expires.as_deref(), Some("2026-11-20"));
    }

    #[test]
    fn formats_picker_lines() {
        let models = parse_models(&backend("openrouter").unwrap(), &openrouter_listing()).unwrap();
        let lines: Vec<String> = models
            .iter()
            .map(|m| m.line(m.id == "typesafe/jev-1.13"))
            .collect();
        assert_eq!(
            lines,
            [
                "~typesafe/jev-latest · free",
                "* typesafe/jev-1.13 · 1M ctx · $0.05/M in · expires 2026-11-20",
                "typesafe/jev-1.14 · 262K ctx · $0.042/M in",
            ]
        );
    }

    #[test]
    fn other_backends_take_ids_and_registry_prices() {
        let wire = json!({ "models": [{ "name": "d1" }, { "name": "d2" }, { "name": "" }] });
        let models = parse_models(&backend("liquid").unwrap(), &wire).unwrap();
        assert_eq!(models[0].line(false), "d1 · $0.04/M in");
        assert_eq!(models[1].line(true), "* d2");
        assert_eq!(models.len(), 2);
        assert!(parse_models(&backend("liquid").unwrap(), &json!({})).is_err());
    }
}
