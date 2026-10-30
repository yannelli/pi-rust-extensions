//! A one-shot localhost HTTP listener for the OAuth redirect (`GET <path>?code=..&state=..`).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

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
    /// The code from the first redirect with the expected state, or an error for an OAuth
    /// error redirect or the login timeout.
    pub async fn wait(&mut self) -> Result<String, String> {
        let Some(rx) = self.rx.as_mut() else {
            return std::future::pending().await;
        };
        let outcome = rx
            .await
            .unwrap_or_else(|_| Err("callback server stopped".into()));
        self.rx = None;
        outcome
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

/// How long one connection may take to send its request.
pub const READ_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the server waits for a valid redirect.
pub const LOGIN_TIMEOUT: Duration = Duration::from_secs(10 * 60);

/// Answers one connection; `Some` ends the login.
async fn handle(mut stream: TcpStream, path: &str, state: &str) -> Option<Result<String, String>> {
    let mut buffer = vec![0u8; 8192];
    let read = tokio::time::timeout(READ_TIMEOUT, stream.read(&mut buffer))
        .await
        .ok()?
        .ok()?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let target = request.split_whitespace().nth(1).unwrap_or_default();
    let (route, query) = target.split_once('?').unwrap_or((target, ""));
    let parsed = parse_authorization_input(&format!("http://localhost{route}?{query}"));
    let error = query.split('&').find_map(|pair| {
        let value = pair.strip_prefix("error=")?;
        Some(super::percent_decode(value))
    });
    let (status, title, outcome) = if route != path {
        ("404 Not Found", "Not found", None)
    } else if parsed.state.as_deref() != Some(state) {
        ("400 Bad Request", "State mismatch", None)
    } else if let Some(error) = error {
        let outcome = Err(format!("OAuth error: {error}"));
        ("400 Bad Request", "Login failed", Some(outcome))
    } else {
        match parsed.code.filter(|c| !c.is_empty()) {
            Some(code) => ("200 OK", "Login complete", Some(Ok(code))),
            None => ("400 Bad Request", "Missing authorization code", None),
        }
    };
    let _ = stream.write_all(page(status, title).as_bytes()).await;
    let _ = stream.shutdown().await;
    outcome
}

/// Binds `127.0.0.1:port`; `None` when the port is taken (the caller then asks for a pasted code).
pub async fn start(port: u16, path: &str, state: &str) -> Option<CallbackServer> {
    start_with_timeout(port, path, state, LOGIN_TIMEOUT).await
}

/// [`start`] with a custom overall timeout. Redirects without the expected state get a 400 page
/// and the server keeps listening.
pub async fn start_with_timeout(
    port: u16,
    path: &str,
    state: &str,
    timeout: Duration,
) -> Option<CallbackServer> {
    let listener = TcpListener::bind(("127.0.0.1", port)).await.ok()?;
    let port = listener.local_addr().ok()?.port();
    let (tx, rx) = oneshot::channel();
    let (path, state) = (path.to_owned(), state.to_owned());
    let task = tokio::spawn(async move {
        let mut connections = JoinSet::new();
        let deadline = tokio::time::sleep(timeout);
        tokio::pin!(deadline);
        let outcome = loop {
            tokio::select! {
                _ = &mut deadline => break Err("Login timed out".to_owned()),
                Some(Ok(Some(outcome))) = connections.join_next() => break outcome,
                accepted = listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        let (path, state) = (path.clone(), state.clone());
                        connections.spawn(async move { handle(stream, &path, &state).await });
                    }
                }
            }
        };
        let _ = tx.send(outcome);
    });
    Some(CallbackServer {
        port,
        rx: Some(rx),
        task,
    })
}
