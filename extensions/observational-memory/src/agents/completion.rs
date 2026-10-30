use std::future::Future;

use serde_json::Value;

/// One-shot text completion: system prompt plus one user prompt.
pub trait Completer: Send + Sync {
    fn complete(
        &self,
        system: &str,
        prompt: &str,
    ) -> impl Future<Output = Result<String, String>> + Send;
}

/// Why a stage call produced no usable reply.
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// The host call failed (no model, no credentials, provider error).
    Model(String),
    /// Two replies in a row did not match the JSON contract.
    BadReply(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Model(message) => f.write_str(message),
            CallError::BadReply(message) => write!(f, "invalid JSON reply: {message}"),
        }
    }
}

/// The JSON object in `text`: code fences stripped, first `{` through last `}`.
pub fn extract_json_object(text: &str) -> Result<Value, String> {
    let start = text.find('{').ok_or("no JSON object in the reply")?;
    let end = text
        .rfind('}')
        .filter(|end| *end > start)
        .ok_or("unterminated JSON object")?;
    let value: Value = serde_json::from_str(&text[start..=end]).map_err(|e| e.to_string())?;
    value
        .is_object()
        .then_some(value)
        .ok_or_else(|| "reply is not a JSON object".to_owned())
}

/// The array under `key`, or an error for the retry note.
pub fn array_field(value: &Value, key: &str) -> Result<Vec<Value>, String> {
    match value.get(key) {
        Some(Value::Array(items)) => Ok(items.clone()),
        Some(_) => Err(format!("\"{key}\" must be an array")),
        None => Err(format!("missing \"{key}\" array")),
    }
}

/// Calls the model and parses the reply; one retry with a correction note on a bad reply.
pub async fn call_json<C, T>(
    completer: &C,
    system: &str,
    prompt: &str,
    parse: impl Fn(&Value) -> Result<T, String>,
) -> Result<T, CallError>
where
    C: Completer,
{
    let mut request = prompt.to_owned();
    let mut last_error = String::new();
    for _ in 0..2 {
        let text = completer
            .complete(system, &request)
            .await
            .map_err(CallError::Model)?;
        match extract_json_object(&text).and_then(|value| parse(&value)) {
            Ok(parsed) => return Ok(parsed),
            Err(error) => {
                request = format!(
                    "{prompt}\n\nYour previous reply could not be used: {error}. Reply again with only the JSON object described in the output format."
                );
                last_error = error;
            }
        }
    }
    Err(CallError::BadReply(last_error))
}

pub fn join_or_empty(items: &[String]) -> String {
    if items.is_empty() {
        "(none yet)".to_owned()
    } else {
        items.join("\n")
    }
}

/// `n.toLocaleString()` for en-US: thousands separators.
pub fn locale(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
