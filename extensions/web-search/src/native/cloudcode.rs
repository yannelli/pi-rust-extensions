//! Cloud Code Assist `v1internal:streamGenerateContent` for Antigravity and Gemini CLI models.
//! The key is JSON `{ "token", "projectId" }`, as pi stores these OAuth credentials.

use anyhow::Result;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Value, json};

use crate::model::{SearchModel, Target};

const ANTIGRAVITY_URL: &str = "https://daily-cloudcode-pa.googleapis.com";
const GEMINI_CLI_URL: &str = "https://cloudcode-pa.googleapis.com";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Antigravity,
    GeminiCli,
}

pub fn flavor(model: &SearchModel) -> Option<Flavor> {
    let ids = [model.provider.as_str(), model.api.as_str()];
    if ids
        .iter()
        .any(|id| matches!(*id, "antigravity" | "google-antigravity"))
    {
        Some(Flavor::Antigravity)
    } else if ids
        .iter()
        .any(|id| matches!(*id, "google-gemini-cli" | "gemini-cli"))
    {
        Some(Flavor::GeminiCli)
    } else {
        None
    }
}

fn runtime_model(flavor: Flavor, id: &str) -> String {
    let mapped = match (flavor, id) {
        (Flavor::Antigravity, "gemini-3.7-flash" | "gemini-3.7-flash-medium") => {
            "gemini-3.7-flash-medium"
        }
        (Flavor::Antigravity, "gemini-3.6-flash") => "gemini-3.6-flash-low",
        (Flavor::Antigravity, "gemini-3.5-flash") => "gemini-3.5-flash-extra-low",
        _ => id,
    };
    mapped.to_owned()
}

/// `(token, projectId)` from the stored credential; a plain string is the token.
pub fn credential(flavor: Flavor, key: Option<&str>) -> (String, Option<String>) {
    let default_project = (flavor == Flavor::Antigravity).then(|| "aicode-consumers".to_owned());
    let Some(key) = key else {
        return (String::new(), default_project);
    };
    match serde_json::from_str::<Value>(key) {
        Ok(Value::Object(map)) => {
            let token = map
                .get("token")
                .and_then(Value::as_str)
                .unwrap_or(key)
                .to_owned();
            let project = map
                .get("projectId")
                .and_then(Value::as_str)
                .map(str::to_owned);
            (token, project.or(default_project))
        }
        _ => (key.to_owned(), default_project),
    }
}

pub fn url(flavor: Flavor, model: &SearchModel) -> String {
    let fallback = if flavor == Flavor::Antigravity {
        ANTIGRAVITY_URL
    } else {
        GEMINI_CLI_URL
    };
    let base = model.base_url.trim().trim_end_matches('/');
    let base = if base.is_empty() { fallback } else { base };
    let base = base.strip_suffix("/v1internal").unwrap_or(base);
    format!("{base}/v1internal:streamGenerateContent?alt=sse")
}

fn platform() -> (&'static str, &'static str, &'static str) {
    let (platform, os) = match std::env::consts::OS {
        "macos" => ("MACOS", "darwin"),
        "windows" => ("WINDOWS", "windows"),
        _ => ("LINUX", "linux"),
    };
    let arch = if std::env::consts::ARCH == "aarch64" {
        "arm64"
    } else {
        "x64"
    };
    (platform, os, arch)
}

pub fn headers(flavor: Flavor, target: &Target) -> Result<HeaderMap> {
    let (token, _) = credential(flavor, target.auth.api_key.as_deref());
    let (platform, os, arch) = platform();
    let ide = if flavor == Flavor::Antigravity {
        "ANTIGRAVITY"
    } else {
        "IDE_UNSPECIFIED"
    };
    let metadata =
        json!({ "ideType": ide, "platform": platform, "pluginType": "GEMINI" }).to_string();
    let agent = match flavor {
        Flavor::Antigravity => format!("antigravity/hub/2.8.0 {os}/{arch}"),
        Flavor::GeminiCli => "google-cloud-sdk vscode_cloudshelleditor/0.1".to_owned(),
    };
    let mut pairs = vec![
        ("content-type".to_owned(), "application/json".to_owned()),
        ("accept".to_owned(), "text/event-stream".to_owned()),
        ("user-agent".to_owned(), agent),
        (
            "x-goog-api-client".to_owned(),
            "google-cloud-sdk vscode_cloudshelleditor/0.1".to_owned(),
        ),
        ("client-metadata".to_owned(), metadata),
    ];
    if !token.is_empty() {
        pairs.push(("authorization".to_owned(), format!("Bearer {token}")));
    }
    pairs.extend(target.session_headers());
    pairs.extend(target.auth.headers.clone());
    let mut map = HeaderMap::new();
    for (name, value) in pairs {
        map.insert(
            HeaderName::from_bytes(name.as_bytes())?,
            HeaderValue::from_str(&value)?,
        );
    }
    Ok(map)
}

/// Wraps a Gemini request in the Cloud Code envelope.
pub fn body(flavor: Flavor, target: &Target, request: &Value) -> Value {
    let (_, project) = credential(flavor, target.auth.api_key.as_deref());
    let mut inner = json!({ "contents": request["contents"] });
    if !request["tools"].is_null() {
        inner["tools"] = request["tools"].clone();
    }
    let user_agent = if flavor == Flavor::Antigravity {
        "antigravity"
    } else {
        "pi-coding-agent"
    };
    let mut envelope = json!({
        "model": runtime_model(flavor, &target.model.id),
        "request": inner,
        "userAgent": user_agent,
    });
    if let Some(project) = project {
        envelope["project"] = json!(project);
    }
    if flavor == Flavor::Antigravity {
        envelope["requestType"] = json!("AGENT");
    }
    envelope
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Auth;

    #[test]
    fn antigravity_envelope_uses_token_project() {
        let raw = json!({ "id": "gemini-3.6-flash", "provider": "google-antigravity", "api": "google-generative-ai" });
        let target = Target {
            model: SearchModel::from_json(raw).unwrap(),
            auth: Auth {
                api_key: Some(r#"{"token":"t1","projectId":"p1"}"#.into()),
                ..Auth::default()
            },
            ..Target::default()
        };
        let flavor = flavor(&target.model).unwrap();
        assert_eq!(
            url(flavor, &target.model),
            format!("{ANTIGRAVITY_URL}/v1internal:streamGenerateContent?alt=sse")
        );
        let body = body(
            flavor,
            &target,
            &json!({ "contents": [], "tools": [{ "google_search": {} }] }),
        );
        assert_eq!(body["project"], "p1");
        assert_eq!(body["model"], "gemini-3.6-flash-low");
        assert_eq!(body["request"]["tools"][0], json!({ "google_search": {} }));
        assert_eq!(
            headers(flavor, &target).unwrap()["authorization"],
            "Bearer t1"
        );
    }
}
