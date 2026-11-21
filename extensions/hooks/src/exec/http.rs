use std::time::Duration;

use serde_json::Value;

use crate::output::RawRun;

/// Claude `allowedHttpHookUrls` and `httpHookAllowedEnvVars`, merged across settings files.
/// `None` when no file sets the key.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HttpPolicy {
    pub urls: Option<Vec<String>>,
    pub env_vars: Option<Vec<String>>,
}

impl HttpPolicy {
    pub fn merge<'a>(docs: impl IntoIterator<Item = &'a Value>) -> Self {
        let mut policy = Self::default();
        for doc in docs {
            for (key, list) in [
                ("allowedHttpHookUrls", &mut policy.urls),
                ("httpHookAllowedEnvVars", &mut policy.env_vars),
            ] {
                if let Some(items) = doc.get(key).and_then(Value::as_array) {
                    let merged = list.get_or_insert_with(Vec::new);
                    for item in items.iter().filter_map(Value::as_str) {
                        if !merged.iter().any(|m| m == item) {
                            merged.push(item.to_owned());
                        }
                    }
                }
            }
        }
        policy
    }

    pub fn url_allowed(&self, url: &str) -> bool {
        let Some(patterns) = &self.urls else {
            return true;
        };
        let Some(url) = normalize_target(url) else {
            return false;
        };
        patterns
            .iter()
            .any(|pattern| glob(&normalize_url(pattern), &url))
    }
}

/// The parsed hook URL without user info, with the host lowercase and without a trailing dot.
fn normalize_target(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let host = host.strip_suffix('.').unwrap_or(host);
    let port = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();
    let query = parsed.query().map(|q| format!("?{q}")).unwrap_or_default();
    Some(format!(
        "{}://{host}{port}{}{query}",
        parsed.scheme(),
        parsed.path()
    ))
}

/// Lowercases the scheme and host of a pattern and drops a trailing dot from the host.
fn normalize_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(end);
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) if !host.ends_with(']') || authority.starts_with('[') => {
            (host, Some(port))
        }
        _ => (authority, None),
    };
    let host = host.strip_suffix('.').unwrap_or(host).to_lowercase();
    let port = port.map(|p| format!(":{p}")).unwrap_or_default();
    format!("{}://{host}{port}{path}", scheme.to_lowercase())
}

/// `*` matches any run of characters, including none.
fn glob(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    let (first, last) = (parts[0], parts[parts.len() - 1]);
    if parts.len() == 1 {
        return pattern == text;
    }
    if !text.starts_with(first) || !text[first.len()..].ends_with(last) {
        return false;
    }
    let mut rest = &text[first.len()..text.len() - last.len()];
    for part in &parts[1..parts.len() - 1] {
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    true
}

/// Headers that reqwest sets from the body; Claude Code ignores them in hook config.
const BODY_HEADERS: &[&str] = &["content-length", "transfer-encoding"];

/// POSTs the event JSON; a 2xx JSON body is read like stdout, other bodies and statuses are errors.
pub async fn run_http(
    handler: &Value,
    body: String,
    timeout: Duration,
    policy: &HttpPolicy,
) -> RawRun {
    let Some(url) = handler.get("url").and_then(Value::as_str) else {
        return RawRun::failed("http hook without url");
    };
    let allowed: Vec<&str> = handler
        .get("allowedEnvVars")
        .and_then(Value::as_array)
        .map(|names| names.iter().filter_map(Value::as_str).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|name| {
            policy
                .env_vars
                .as_ref()
                .is_none_or(|list| list.iter().any(|n| n == name))
        })
        .collect();
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
            if BODY_HEADERS.contains(&name.to_lowercase().as_str()) {
                continue;
            }
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
    if status.is_success() && crate::output::json_output(&text).is_none() && !text.trim().is_empty()
    {
        RawRun::failed("HTTP 2xx body is not a JSON object")
    } else if status.is_success() {
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

    #[test]
    fn url_patterns_merge_and_match() {
        let user = json!({ "allowedHttpHookUrls": ["https://hooks.example.com/*"] });
        let project = json!({ "allowedHttpHookUrls": ["http://localhost:*", "https://hooks.example.com/*"],
            "httpHookAllowedEnvVars": ["TOKEN"] });
        let policy = HttpPolicy::merge([&user, &project]);
        assert_eq!(policy.urls.as_ref().unwrap().len(), 2);
        assert!(policy.url_allowed("https://HOOKS.example.com./a/b"));
        assert!(policy.url_allowed("http://localhost:8080/x"));
        assert!(!policy.url_allowed("https://evil.example.com/hooks.example.com/"));
        assert!(!policy.url_allowed("http://localhost.evil.com/"));
        assert!(!policy.url_allowed("http://localhost:1@evil.example/"));
        assert!(!policy.url_allowed("not a url"));
        assert_eq!(policy.env_vars, Some(vec!["TOKEN".to_owned()]));
        assert!(HttpPolicy::default().url_allowed("https://any.example/"));
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
        let run = run_http(
            &handler,
            body,
            Duration::from_secs(5),
            &HttpPolicy::default(),
        )
        .await;
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
            &HttpPolicy::default(),
        )
        .await;
        assert_eq!(run.code, Some(1));
        assert!(run.stderr.contains("500"));
    }
}
