use std::fmt;

use serde_json::{Value, json};

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub mod code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const CANCELLED: i64 = -32800;
    pub const NO_UI: i64 = -32001;
    pub const NO_CONTEXT: i64 = -32002;
    pub const NOT_TRUSTED: i64 = -32003;
}

/// A JSON-RPC error. `?` converts any `std::error::Error` or `anyhow::Error` into
/// an internal error (-32603). It does not implement `std::error::Error` itself.
#[derive(Clone, PartialEq)]
pub struct Error {
    code: i64,
    message: String,
    data: Option<Box<Value>>,
}

impl Error {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    pub fn msg(message: impl fmt::Display) -> Self {
        Self::new(code::INTERNAL_ERROR, message.to_string())
    }

    pub fn invalid_params(message: impl fmt::Display) -> Self {
        Self::new(code::INVALID_PARAMS, message.to_string())
    }

    pub fn method_not_found(method: &str) -> Self {
        Self::new(code::METHOD_NOT_FOUND, format!("unknown method: {method}"))
    }

    pub fn cancelled() -> Self {
        Self::new(code::CANCELLED, "request cancelled")
    }

    pub fn closed() -> Self {
        Self::new(code::INTERNAL_ERROR, "connection to pi closed")
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(Box::new(data));
        self
    }

    pub fn code(&self) -> i64 {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn data(&self) -> Option<&Value> {
        self.data.as_deref()
    }

    pub fn is_cancelled(&self) -> bool {
        self.code == code::CANCELLED
    }

    pub fn is_no_ui(&self) -> bool {
        self.code == code::NO_UI
    }

    pub fn is_no_context(&self) -> bool {
        self.code == code::NO_CONTEXT
    }

    pub fn into_anyhow(self) -> anyhow::Error {
        anyhow::Error::msg(self.to_string())
    }

    pub(crate) fn to_json(&self) -> Value {
        let mut value = json!({ "code": self.code, "message": self.message });
        if let Some(data) = &self.data {
            value["data"] = data.as_ref().clone();
        }
        value
    }

    pub(crate) fn from_json(value: &Value) -> Self {
        let code = value
            .get("code")
            .and_then(Value::as_i64)
            .unwrap_or(code::INTERNAL_ERROR);
        let message = value
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        Self {
            code,
            message: message.to_owned(),
            data: value.get("data").cloned().map(Box::new),
        }
    }
}

impl<E: Into<anyhow::Error>> From<E> for Error {
    fn from(err: E) -> Self {
        Self::msg(format!("{:#}", err.into()))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.message, self.code)
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Error")
            .field("code", &self.code)
            .field("message", &self.message)
            .field("data", &self.data)
            .finish()
    }
}

/// Returns early with an internal error built from a format string.
#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => {
        return ::std::result::Result::Err($crate::Error::msg(::std::format!($($arg)*)))
    };
}
