//! A scripted HTTP/1.1 server on 127.0.0.1 for tests of the TypeSafe client.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

impl Reply {
    pub fn json(status: u16, body: &Value) -> Self {
        Self {
            status,
            body: body.to_string(),
            headers: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }
}

#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub authorization: Option<String>,
    pub body: String,
}

#[derive(Clone)]
pub struct MockApi {
    pub url: String,
    replies: Arc<Mutex<VecDeque<Reply>>>,
    recorded: Arc<Mutex<Vec<Recorded>>>,
}

impl MockApi {
    pub async fn start(replies: Vec<Reply>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
        let url = format!("http://{}", listener.local_addr().expect("mock addr"));
        let api = Self {
            url,
            replies: Arc::new(Mutex::new(replies.into())),
            recorded: Arc::new(Mutex::new(Vec::new())),
        };
        let server = api.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let server = server.clone();
                tokio::spawn(async move { server.serve(stream).await });
            }
        });
        api
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.recorded.lock().expect("recorded").clone()
    }

    async fn serve(&self, stream: tokio::net::TcpStream) {
        let mut reader = BufReader::new(stream);
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            let mut parts = line.split_whitespace();
            let (method, path) = (
                parts.next().unwrap_or_default().to_owned(),
                parts.next().unwrap_or_default().to_owned(),
            );
            let (mut length, mut authorization) = (0usize, None);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).await.unwrap_or(0) == 0 {
                    return;
                }
                let header = header.trim_end();
                if header.is_empty() {
                    break;
                }
                let (name, value) = header.split_once(':').unwrap_or((header, ""));
                match name.to_ascii_lowercase().as_str() {
                    "content-length" => length = value.trim().parse().unwrap_or(0),
                    "authorization" => authorization = Some(value.trim().to_owned()),
                    _ => {}
                }
            }
            let mut body = vec![0; length];
            if reader.read_exact(&mut body).await.is_err() {
                return;
            }
            let body = String::from_utf8_lossy(&body).into_owned();
            self.recorded.lock().expect("recorded").push(Recorded {
                method,
                path,
                authorization,
                body,
            });
            let reply = self.replies.lock().expect("replies").pop_front();
            let reply = reply.unwrap_or_else(|| Reply::json(500, &Value::Null));
            let mut head = format!(
                "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\n",
                reply.status,
                reply.body.len()
            );
            for (name, value) in &reply.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str("\r\n");
            let stream = reader.get_mut();
            if stream.write_all(head.as_bytes()).await.is_err()
                || stream.write_all(reply.body.as_bytes()).await.is_err()
            {
                return;
            }
        }
    }
}
