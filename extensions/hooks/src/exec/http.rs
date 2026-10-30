use std::time::Duration;

use serde_json::Value;

use crate::output::RawRun;

/// POSTs the event JSON; a 2xx body is read like stdout, other statuses are non-blocking errors.
pub async fn run_http(handler: &Value, body: String, timeout: Duration) -> RawRun {
    let Some(url) = handler.get("url").and_then(Value::as_str) else {
        return RawRun::failed("http hook without url");
    };
    let allowed: Vec<&str> = handler
        .get("allowedEnvVars")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let client = match reqwest::Client::builder().timeout(timeout).build() {
        Ok(client) => client,
        Err(err) => return RawRun::failed(format!("http client: {err}")),
    };
    let mut request = client
        .post(url)
        .header("content-type", "application/json")
        .body(body);
    if let Some(headers) = handler.get("headers").and_then(Value::as_object) {
        for (name, value) in headers {
            if let Some(value) = value.as_str() {
                request =
                    request.header(name, expand_env(value, &allowed, |n| std::env::var(n).ok()));
            }
        }
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) if err.is_timeout() => {
            return RawRun::failed(format!("timed out after {}s", timeout.as_secs_f64()));
        }
        Err(err) => return RawRun::failed(format!("http error: {err}")),
    };
    let status = response.status();
    let text = response.text().await.unwrap_or_default();
    if status.is_success() {
        RawRun::exit(0, text, "")
    } else {
        RawRun::exit(1, "", format!("HTTP {status}: {text}"))
    }
}

/// Expands `$VAR` and `${VAR}` for names in `allowed`; other names become empty.
pub fn expand_env(text: &str, allowed: &[&str], lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        let after = &rest[at + 1..];
        let (name, used) = if let Some(braced) = after.strip_prefix('{') {
            match braced.find('}') {
                Some(end) => (&braced[..end], end + 2),
                None => ("", 0),
            }
        } else {
            let end = after
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(after.len());
            (&after[..end], end)
        };
        if name.is_empty() {
            out.push('$');
            rest = after;
            continue;
        }
        if allowed.contains(&name) {
            out.push_str(&lookup(name).unwrap_or_default());
        }
        rest = &after[used..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn expands_only_allowed_vars() {
        let lookup = |name: &str| Some(format!("<{name}>"));
        assert_eq!(
            expand_env("Bearer $TOKEN", &["TOKEN"], lookup),
            "Bearer <TOKEN>"
        );
        assert_eq!(
            expand_env("${TOKEN}x $SECRET!", &["TOKEN"], lookup),
            "<TOKEN>x !"
        );
        assert_eq!(expand_env("cost $ 5", &[], lookup), "cost $ 5");
    }

    async fn serve_once(status: &str, body: &str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        let reply = format!(
            "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut seen = Vec::new();
            let mut buf = [0u8; 4096];
            while !String::from_utf8_lossy(&seen).contains("\"hook_event_name\"") {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                seen.extend_from_slice(&buf[..n]);
            }
            socket.write_all(reply.as_bytes()).await.unwrap();
            String::from_utf8_lossy(&seen).into_owned()
        });
        (url, task)
    }

    #[tokio::test]
    async fn posts_json_and_reads_body() {
        let (url, server) = serve_once("200 OK", r#"{"decision":"block","reason":"r"}"#).await;
        let handler = json!({ "type": "http", "url": url, "headers": { "x-test": "a$NOPE" } });
        let body = json!({ "hook_event_name": "Stop" }).to_string();
        let run = run_http(&handler, body, Duration::from_secs(5)).await;
        assert_eq!(run.code, Some(0));
        assert_eq!(run.stdout, r#"{"decision":"block","reason":"r"}"#);
        let request = server.await.unwrap();
        assert!(request.starts_with("POST /hook"));
        assert!(request.contains("x-test: a\r\n"));
    }

    #[tokio::test]
    async fn non_2xx_is_an_error() {
        let (url, _server) = serve_once("500 Internal Server Error", "bad").await;
        let run = run_http(
            &json!({ "url": url }),
            json!({ "hook_event_name": "x" }).to_string(),
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(run.code, Some(1));
        assert!(run.stderr.contains("500"));
    }
}
