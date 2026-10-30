//! A one-shot localhost HTTP listener for the OAuth redirect (`GET <path>?code=..&state=..`).

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use super::parse_authorization_input;

pub struct CallbackServer {
    pub port: u16,
    rx: Option<oneshot::Receiver<Result<String, String>>>,
    task: JoinHandle<()>,
}

impl Drop for CallbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl CallbackServer {
    /// The code from the redirect, or an error for a state mismatch or an OAuth error.
    pub async fn wait(&mut self) -> Result<String, String> {
        match self.rx.take() {
            Some(rx) => rx
                .await
                .unwrap_or_else(|_| Err("callback server stopped".into())),
            None => std::future::pending().await,
        }
    }
}

fn page(status: &str, title: &str) -> String {
    let body = format!(
        "<!doctype html><html><body><h1>{title}</h1><p>You can close this window.</p></body></html>"
    );
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Binds `127.0.0.1:port`; `None` when the port is taken (the caller then asks for a pasted code).
pub async fn start(port: u16, path: &str, state: &str) -> Option<CallbackServer> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.ok()?;
    let port = listener.local_addr().ok()?.port();
    let (tx, rx) = oneshot::channel();
    let (path, state) = (path.to_owned(), state.to_owned());
    let task = tokio::spawn(async move {
        let mut tx = Some(tx);
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut buffer = vec![0u8; 8192];
            let read = stream.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..read]);
            let target = request
                .split_whitespace()
                .nth(1)
                .unwrap_or_default()
                .to_owned();
            let (route, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
            if route != path {
                let _ = stream
                    .write_all(page("404 Not Found", "Not found").as_bytes())
                    .await;
                continue;
            }
            let parsed = parse_authorization_input(&format!("http://localhost{route}?{query}"));
            let error = query.split('&').any(|pair| pair.starts_with("error="));
            let outcome = match (parsed.code, parsed.state) {
                _ if error => Err(format!("OAuth error: {query}")),
                (_, Some(got)) if got != state => Err("State mismatch".to_owned()),
                (Some(code), _) if !code.is_empty() => Ok(code),
                _ => Err("Missing authorization code".to_owned()),
            };
            let (status, title) = match &outcome {
                Ok(_) => ("200 OK", "Login complete"),
                Err(_) => ("400 Bad Request", "Login failed"),
            };
            let _ = stream.write_all(page(status, title).as_bytes()).await;
            let _ = stream.shutdown().await;
            if let Some(tx) = tx.take() {
                let _ = tx.send(outcome);
            }
            break;
        }
    });
    Some(CallbackServer {
        port,
        rx: Some(rx),
        task,
    })
}
