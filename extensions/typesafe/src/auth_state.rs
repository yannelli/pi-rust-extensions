use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::backends::{Backend, DEFAULT_BACKEND};
use crate::credentials::{KeySituation, write_private};
use crate::errors::{Code, TsError};

#[derive(Debug, Clone, PartialEq)]
pub struct Failure {
    pub code: Code,
    pub message: String,
    pub at: String,
    pub status: Option<u16>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AuthRecord {
    pub verified_at: Option<String>,
    pub last_failure: Option<Failure>,
}

impl AuthRecord {
    pub fn rejected(&self) -> bool {
        self.last_failure
            .as_ref()
            .is_some_and(|f| f.code == Code::Http && matches!(f.status, Some(401 | 403)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warning,
    Error,
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// `auth-state.json`: the last verification and failure, owner-only and best effort.
#[derive(Debug, Clone)]
pub struct AuthState {
    pub path: PathBuf,
}

impl AuthState {
    pub fn new(dir: &Path) -> Self {
        Self {
            path: dir.join("auth-state.json"),
        }
    }

    pub fn read(&self) -> AuthRecord {
        let Ok(text) = fs::read_to_string(&self.path) else {
            return AuthRecord::default();
        };
        let raw: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
        let short = |v: &Value| v.as_str().filter(|s| s.len() <= 40).map(str::to_owned);
        let failure = raw.get("lastFailure").and_then(|f| {
            Some(Failure {
                code: Code::parse(f.get("code")?.as_str()?)?,
                message: f.get("message")?.as_str()?.chars().take(300).collect(),
                at: short(f.get("at")?)?,
                status: f
                    .get("status")
                    .and_then(Value::as_u64)
                    .and_then(|s| u16::try_from(s).ok()),
            })
        });
        AuthRecord {
            verified_at: raw.get("verifiedAt").and_then(short),
            last_failure: failure,
        }
    }

    fn write(&self, record: &AuthRecord) {
        let mut body = json!({ "version": 1 });
        if let Some(at) = &record.verified_at {
            body["verifiedAt"] = json!(at);
        }
        if let Some(f) = &record.last_failure {
            body["lastFailure"] =
                json!({ "code": f.code.as_str(), "message": f.message, "at": f.at });
            if let Some(status) = f.status {
                body["lastFailure"]["status"] = json!(status);
            }
        }
        if let Ok(text) = serde_json::to_string_pretty(&body) {
            let _ = write_private(&self.path, &format!("{text}\n"));
        }
    }

    /// The key was accepted; clears the last failure.
    pub fn record_verified(&self) {
        self.write(&AuthRecord {
            verified_at: Some(now_iso()),
            last_failure: None,
        });
    }

    /// Keeps the verification time so a recovered key stays known.
    pub fn record_failure(&self, error: &TsError) {
        let verified_at = self.read().verified_at;
        self.write(&AuthRecord {
            verified_at,
            last_failure: Some(Failure {
                code: error.code,
                message: error.message.clone(),
                at: now_iso(),
                status: error.status,
            }),
        });
    }

    pub fn clear(&self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// One line and a level for status output, as upstream `describeAuth`.
pub fn describe(
    backend: &Backend,
    situation: &KeySituation,
    record: &AuthRecord,
) -> (Level, String) {
    let label = format!("{} key", backend.label);
    let since = record.last_failure.as_ref().map_or(String::new(), |f| {
        format!(" Last failure: {} ({})", f.message, f.at)
    });
    match situation {
        KeySituation::Missing => {
            let login = match backend.name {
                DEFAULT_BACKEND => "/typesafe login".to_owned(),
                name => format!("/typesafe login {name}"),
            };
            let how = format!("a key is configured ({login} or {})", backend.key_env);
            (
                Level::Error,
                format!("{label}: missing — every Jev judgment is skipped until {how}.{since}"),
            )
        }
        KeySituation::Unusable { reason } => (
            Level::Error,
            format!(
                "{label}: unusable ({reason}) — judgments are skipped until the key is fixed.{since}"
            ),
        ),
        _ if record.rejected() => (
            Level::Error,
            format!("{label}: {} was rejected.{since}", situation.label()),
        ),
        _ => match &record.verified_at {
            None => (
                Level::Warning,
                format!(
                    "{label}: {} (not verified yet — the first request proves it).{since}",
                    situation.label()
                ),
            ),
            Some(at) => (
                Level::Ok,
                format!("{label}: {} (verified {at}).{since}", situation.label()),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::backend;
    use crate::testutil::TempDir;

    #[test]
    fn records_and_describes() {
        let dir = TempDir::new();
        let state = AuthState::new(dir.path());
        let typesafe = backend("typesafe").unwrap();
        let stored = KeySituation::Stored { key: "k".into() };
        let (level, text) = describe(&typesafe, &stored, &state.read());
        assert_eq!(level, Level::Warning);
        assert!(text.contains("/typesafe login (not verified yet"));
        state.record_verified();
        assert_eq!(describe(&typesafe, &stored, &state.read()).0, Level::Ok);
        state.record_failure(&TsError::http(401, None, Some(&typesafe)));
        let record = state.read();
        assert!(record.verified_at.is_some() && record.rejected());
        let (level, text) = describe(&typesafe, &stored, &record);
        assert_eq!(level, Level::Error);
        assert!(text.contains("was rejected. Last failure: TypeSafe returned HTTP 401."));
        let (_, missing) = describe(
            &backend("liquid").unwrap(),
            &KeySituation::Missing,
            &AuthRecord::default(),
        );
        assert!(missing.contains("/typesafe login liquid or LIQUID_API_KEY"));
        state.clear();
        assert_eq!(state.read(), AuthRecord::default());
    }
}
