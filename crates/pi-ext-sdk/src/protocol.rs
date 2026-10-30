use serde_json::{Map, Value, json};

use crate::error::{Error, code};

pub const PROTOCOL_VERSION: i64 = 1;

#[derive(Debug)]
pub(crate) enum Incoming {
    Request {
        id: Value,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: Value,
        result: Result<Value, Error>,
    },
}

pub(crate) fn parse(line: &str) -> Result<Incoming, (Value, Error)> {
    let value: Value = serde_json::from_str(line)
        .map_err(|err| (Value::Null, Error::new(code::PARSE_ERROR, err.to_string())))?;
    let Value::Object(mut obj) = value else {
        return Err(invalid(Value::Null, "message is not an object"));
    };
    let id = obj.remove("id");
    let method = obj.remove("method");
    let params = obj.remove("params").unwrap_or(Value::Null);
    match (id, method) {
        (Some(id), Some(Value::String(method))) => Ok(Incoming::Request { id, method, params }),
        (None, Some(Value::String(method))) => Ok(Incoming::Notification { method, params }),
        (Some(id), None) => Ok(Incoming::Response {
            result: response_result(&mut obj),
            id,
        }),
        (id, _) => Err(invalid(id.unwrap_or(Value::Null), "missing method")),
    }
}

fn response_result(obj: &mut Map<String, Value>) -> Result<Value, Error> {
    match obj.remove("error") {
        Some(err) if !err.is_null() => Err(Error::from_json(&err)),
        _ => Ok(obj.remove("result").unwrap_or(Value::Null)),
    }
}

fn invalid(id: Value, message: &str) -> (Value, Error) {
    (id, Error::new(code::INVALID_REQUEST, message))
}

pub(crate) fn request(id: u64, method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }).to_string()
}

pub(crate) fn notification(method: &str, params: Value) -> String {
    json!({ "jsonrpc": "2.0", "method": method, "params": params }).to_string()
}

pub(crate) fn response(id: &Value, result: Result<Value, Error>) -> String {
    match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err(err) => json!({ "jsonrpc": "2.0", "id": id, "error": err.to_json() }),
    }
    .to_string()
}

/// Key used to match ids of either JSON type (`1` and `"1"` stay distinct).
pub(crate) fn id_key(id: &Value) -> String {
    id.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_messages() {
        let req = parse(r#"{"jsonrpc":"2.0","id":3,"method":"event","params":{}}"#).unwrap();
        assert!(matches!(req, Incoming::Request { ref method, .. } if method == "event"));
        let note = parse(r#"{"jsonrpc":"2.0","method":"exit"}"#).unwrap();
        assert!(matches!(
            note,
            Incoming::Notification {
                params: Value::Null,
                ..
            }
        ));
        let err = parse(r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32001,"message":"no UI"}}"#);
        let Ok(Incoming::Response {
            result: Err(err), ..
        }) = err
        else {
            panic!("expected error response");
        };
        assert!(err.is_no_ui());
        assert!(parse("not json").is_err());
    }
}
