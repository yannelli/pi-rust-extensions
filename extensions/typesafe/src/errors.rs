use std::fmt;

use crate::backends::{Backend, TYPESAFE_KEY_ENV};

/// Error kinds, as the upstream `TypeSafeIntegrationError.code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    Configuration,
    Validation,
    Budget,
    Aborted,
    Timeout,
    Http,
    Connection,
    Response,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Configuration => "configuration",
            Code::Validation => "validation",
            Code::Budget => "budget",
            Code::Aborted => "aborted",
            Code::Timeout => "timeout",
            Code::Http => "http",
            Code::Connection => "connection",
            Code::Response => "response",
        }
    }

    pub fn parse(text: &str) -> Option<Code> {
        [
            Code::Configuration,
            Code::Validation,
            Code::Budget,
            Code::Aborted,
            Code::Timeout,
            Code::Http,
            Code::Connection,
            Code::Response,
        ]
        .into_iter()
        .find(|code| code.as_str() == text)
    }
}

/// A message safe to display: no upstream bodies, keys, or submitted state.
#[derive(Debug, Clone, PartialEq)]
pub struct TsError {
    pub code: Code,
    pub message: String,
    pub status: Option<u16>,
}

pub type TsResult<T> = std::result::Result<T, TsError>;

impl TsError {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
        }
    }

    pub fn configuration(message: impl Into<String>) -> Self {
        Self::new(Code::Configuration, message)
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::new(Code::Validation, message)
    }

    pub fn aborted() -> Self {
        Self::new(
            Code::Aborted,
            "TypeSafe request cancelled; an already submitted request may still be billed.",
        )
    }

    pub fn timeout() -> Self {
        Self::new(
            Code::Timeout,
            "TypeSafe request timed out; it was not retried and may still be billed.",
        )
    }

    pub fn connection() -> Self {
        Self::new(
            Code::Connection,
            "Could not complete the TypeSafe connection. No automatic retry was made.",
        )
    }

    pub fn response() -> Self {
        Self::new(
            Code::Response,
            "TypeSafe returned an unreadable or unexpected response.",
        )
    }

    /// An HTTP failure with the advice for its status. `retry_after` is a numeric `Retry-After`.
    pub fn http(status: u16, retry_after: Option<&str>, backend: Option<&Backend>) -> Self {
        let key_env = backend.map_or(TYPESAFE_KEY_ENV, |b| b.key_env);
        let openrouter = backend.is_some_and(|b| b.name == "openrouter");
        let retry = retry_after
            .map(str::trim)
            .and_then(|raw| raw.parse::<u64>().ok())
            .filter(|_| status == 429);
        let advice = match status {
            401 => format!("Check {key_env}."),
            402 if openrouter => {
                "Insufficient credits. Add credits at https://openrouter.ai/credits.".to_owned()
            }
            402 => "Check your account balance.".to_owned(),
            403 => "Check your account access and model permissions.".to_owned(),
            429 => format!(
                "Check your account quota and try again later.{}",
                retry.map_or(String::new(), |s| format!(" Retry after {s} seconds."))
            ),
            400 | 422 => "Check the question format and model limits.".to_owned(),
            _ => "Try again later or check the service status.".to_owned(),
        };
        Self {
            code: Code::Http,
            message: format!(
                "TypeSafe returned HTTP {status}. {advice} No automatic retry was made."
            ),
            status: Some(status),
        }
    }

    /// 401 and 403 mean the key itself was refused.
    pub fn is_rejected_key(&self) -> bool {
        self.code == Code::Http && matches!(self.status, Some(401 | 403))
    }
}

impl fmt::Display for TsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<TsError> for pi_ext_sdk::Error {
    fn from(error: TsError) -> Self {
        pi_ext_sdk::Error::msg(error.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backends::backend;

    #[test]
    fn http_advice_follows_status() {
        let openrouter = backend("openrouter").unwrap();
        assert!(
            TsError::http(401, None, Some(&openrouter))
                .message
                .contains("OPENROUTER_API_KEY")
        );
        assert!(
            TsError::http(401, None, None)
                .message
                .contains("TYPESAFE_API_KEY")
        );
        assert!(
            TsError::http(402, None, Some(&openrouter))
                .message
                .contains("openrouter.ai/credits")
        );
        let limited = TsError::http(429, Some(" 12 "), None);
        assert!(
            limited
                .message
                .ends_with("Retry after 12 seconds. No automatic retry was made.")
        );
        assert!(
            !TsError::http(429, Some("Wed, 21 Oct"), None)
                .message
                .contains("Retry after")
        );
        assert!(TsError::http(403, None, None).is_rejected_key());
        assert!(!TsError::http(500, None, None).is_rejected_key());
    }
}
