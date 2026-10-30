use crate::errors::{TsError, TsResult};

pub const DEFAULT_BACKEND: &str = "typesafe";
pub const TYPESAFE_KEY_ENV: &str = "TYPESAFE_API_KEY";
pub const BACKEND_ENV: &str = "PI_TYPESAFE_BACKEND";
pub const MODEL_ENV: &str = "PI_TYPESAFE_MODEL";
pub const BASE_URL_ENV: &str = "PI_TYPESAFE_BASE_URL";
pub const NAMES: [&str; 5] = ["typesafe", "openrouter", "commandcode", "liquid", "subq"];

const SYSTEMONE_PATH: &str = "/v1/systemone";
const MODELS_PATH: &str = "/v1/models";

/// Request limits a backend accepts; the schema bounds are the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_questions: usize,
    pub min_choice_options: usize,
    pub max_choice_options: usize,
    pub max_score_levels: usize,
}

pub const DEFAULT_LIMITS: Limits = Limits {
    max_questions: 32,
    min_choice_options: 1,
    max_choice_options: 64,
    max_score_levels: 32,
};

/// One judgment backend of the upstream registry.
#[derive(Debug, Clone, PartialEq)]
pub struct Backend {
    pub name: &'static str,
    pub label: &'static str,
    pub host: String,
    pub key_env: &'static str,
    pub path: &'static str,
    pub models_path: &'static str,
    pub models_field: &'static str,
    pub models_id_field: &'static str,
    pub models_verify_key: bool,
    pub default_model: &'static str,
    pub limits: Limits,
    pub prices: &'static [(&'static str, f64)],
}

/// The registry entry for `name`; a `configuration` error names the valid backends.
pub fn backend(name: &str) -> TsResult<Backend> {
    let base = Backend {
        name: DEFAULT_BACKEND,
        label: "TypeSafe",
        host: "https://api.typesafe.ai".to_owned(),
        key_env: TYPESAFE_KEY_ENV,
        path: SYSTEMONE_PATH,
        models_path: MODELS_PATH,
        models_field: "models",
        models_id_field: "name",
        models_verify_key: true,
        default_model: "jev-latest",
        limits: DEFAULT_LIMITS,
        prices: &[],
    };
    let entry = match name {
        "typesafe" => base,
        "openrouter" => Backend {
            name: "openrouter",
            label: "OpenRouter",
            host: "https://openrouter.ai".to_owned(),
            key_env: "OPENROUTER_API_KEY",
            path: "/api/alpha/decisions",
            models_path: "/api/v1/models",
            models_field: "data",
            models_id_field: "id",
            models_verify_key: false,
            default_model: "typesafe/jev-1.13",
            ..base
        },
        "commandcode" => Backend {
            name: "commandcode",
            label: "Command Code",
            host: "https://api.commandcode.ai".to_owned(),
            key_env: "COMMANDCODE_API_KEY",
            path: "/provider/v1/systemone",
            models_path: "/provider/v1/models",
            models_field: "data",
            models_id_field: "id",
            models_verify_key: false,
            default_model: "typesafe/jev",
            ..base
        },
        "liquid" => Backend {
            name: "liquid",
            label: "Liquid AI",
            host: "https://api.liquid.ai".to_owned(),
            key_env: "LIQUID_API_KEY",
            path: "/decisions/v1/systemone",
            models_path: "/decisions/v1/models",
            default_model: "d1:free",
            prices: &[("d1", 0.04), ("d1:free", 0.0)],
            ..base
        },
        "subq" => Backend {
            name: "subq",
            label: "SubQ",
            host: "https://api.subq.ai".to_owned(),
            key_env: "SUBQ_API_KEY",
            models_field: "data",
            models_id_field: "id",
            models_verify_key: false,
            default_model: "subq-snap-preview",
            limits: Limits {
                max_questions: 16,
                min_choice_options: 2,
                max_score_levels: 10,
                ..DEFAULT_LIMITS
            },
            prices: &[("subq-snap-preview", 0.05)],
            ..base
        },
        _ => {
            return Err(TsError::configuration(format!(
                "Unknown judgment backend \"{name}\". Valid backends: {}.",
                NAMES.join(", ")
            )));
        }
    };
    Ok(entry)
}

impl Backend {
    /// Scheme-less `host[:port]`, for consent text.
    pub fn host_name(&self) -> &str {
        let rest = self.host.split_once("://").map_or(&*self.host, |(_, r)| r);
        rest.trim_end_matches('/')
    }

    /// The model id a request carries: the caller's id in this backend's form, or the default.
    pub fn sent_model(&self, model: Option<&str>) -> String {
        map_model(self.name, model.unwrap_or(self.default_model))
    }

    /// USD per million input tokens for the model as sent, when the registry names one.
    pub fn price(&self, model: &str) -> Option<f64> {
        self.prices
            .iter()
            .find(|(m, _)| *m == model)
            .map(|(_, p)| *p)
    }

    pub fn systemone_url(&self) -> String {
        format!("{}{}", self.host, self.path)
    }

    pub fn models_url(&self) -> String {
        format!("{}{}", self.host, self.models_path)
    }
}

/// OpenRouter routes bare Jev ids under the `typesafe` author; every other id passes unchanged.
pub fn map_model(backend: &str, model: &str) -> String {
    if model.contains('/') || backend != "openrouter" {
        return model.to_owned();
    }
    if model == "jev-latest" {
        return "~typesafe/jev-latest".to_owned();
    }
    let version = model.strip_prefix("jev-").and_then(|rest| {
        let parts: Vec<&str> = rest.split('.').collect();
        let numeric = parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
        (numeric && (parts.len() == 2 || parts.len() == 3))
            .then(|| format!("{}.{}", parts[0], parts[1]))
    });
    version.map_or_else(|| model.to_owned(), |v| format!("typesafe/jev-{v}"))
}

/// A base URL override: an https origin, or http on a loopback host. No path, query, or user info.
pub fn validate_base_url(raw: &str) -> TsResult<String> {
    let refuse = || {
        TsError::configuration(format!(
            "{BASE_URL_ENV} must be an absolute https: origin with no user info, path, query, or fragment (http: is allowed only for localhost, 127.0.0.0/8, and [::1])."
        ))
    };
    let raw = raw.trim().trim_end_matches('/');
    let (scheme, rest) = raw.split_once("://").ok_or_else(refuse)?;
    if rest.is_empty() || rest.contains(['/', '?', '#', '@']) {
        return Err(refuse());
    }
    let host = if rest.starts_with('[') {
        rest.split_once(']').map_or(rest, |(h, _)| h).to_owned() + "]"
    } else {
        rest.split(':').next().unwrap_or_default().to_owned()
    };
    let loopback = host == "localhost"
        || host == "[::1]"
        || host.strip_prefix("127.").is_some_and(|tail| {
            let parts: Vec<&str> = tail.split('.').collect();
            parts.len() == 3 && parts.iter().all(|p| p.parse::<u8>().is_ok())
        });
    match scheme {
        "https" => Ok(raw.to_owned()),
        "http" if loopback => Ok(raw.to_owned()),
        _ => Err(refuse()),
    }
}

/// The backend named by `PI_TYPESAFE_BACKEND` (blank means the default) with any base URL override.
pub fn from_values(name: Option<&str>, base_url: Option<&str>) -> TsResult<Backend> {
    let name = name
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .unwrap_or(DEFAULT_BACKEND);
    let mut entry = backend(name)?;
    if let Some(url) = base_url.map(str::trim).filter(|u| !u.is_empty()) {
        entry.host = validate_base_url(url)?;
    }
    Ok(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_openrouter_models() {
        assert_eq!(
            map_model("openrouter", "jev-latest"),
            "~typesafe/jev-latest"
        );
        assert_eq!(map_model("openrouter", "jev-1.13.2"), "typesafe/jev-1.13");
        assert_eq!(map_model("openrouter", "jev-1.13"), "typesafe/jev-1.13");
        assert_eq!(map_model("openrouter", "jev-x"), "jev-x");
        assert_eq!(map_model("openrouter", "vendor/m"), "vendor/m");
        assert_eq!(map_model("typesafe", "jev-1.13"), "jev-1.13");
        assert_eq!(
            backend("openrouter").unwrap().sent_model(None),
            "typesafe/jev-1.13"
        );
    }

    #[test]
    fn resolves_registry_and_overrides() {
        assert!(
            backend("toString")
                .unwrap_err()
                .message
                .contains("Valid backends: typesafe")
        );
        let subq = from_values(Some(" subq "), None).unwrap();
        assert_eq!(subq.limits.max_questions, 16);
        assert_eq!(subq.price("subq-snap-preview"), Some(0.05));
        assert_eq!(
            from_values(None, None).unwrap().host_name(),
            "api.typesafe.ai"
        );
        let local = from_values(None, Some("http://127.0.0.1:8080/")).unwrap();
        assert_eq!(local.systemone_url(), "http://127.0.0.1:8080/v1/systemone");
        assert!(validate_base_url("http://example.com").is_err());
        assert!(validate_base_url("https://u:p@example.com").is_err());
        assert!(validate_base_url("https://example.com/path").is_err());
        assert!(validate_base_url("http://[::1]:9").is_ok());
        assert!(validate_base_url("https://gw.example.com:8443").is_ok());
    }
}
