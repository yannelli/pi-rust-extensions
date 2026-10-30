//! A mock Ollama on 127.0.0.1: `/api/tags`, `/api/show`, streaming `/api/pull`, `/api/delete`,
//! and a streaming `/v1/chat/completions`.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

#[derive(Default)]
struct State {
    models: Vec<String>,
    requests: Vec<(String, String, Value)>,
}

pub struct MockOllama {
    url: String,
    state: Arc<Mutex<State>>,
}

fn show(name: &str) -> Value {
    if name.starts_with("nomic-embed") {
        return json!({"capabilities": ["embedding"], "model_info": {"general.architecture": "nomic-bert", "nomic-bert.context_length": 2048}});
    }
    json!({
        "capabilities": ["completion", "tools", "thinking"],
        "details": {"family": "qwen3", "parameter_size": "8.2B", "quantization_level": "Q4_K_M"},
        "model_info": {"general.architecture": "qwen3", "qwen3.context_length": 40960}
    })
}

impl MockOllama {
    pub fn start(models: &[&str]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State {
            models: models.iter().map(|m| m.to_string()).collect(),
            requests: Vec::new(),
        }));
        let shared = state.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let state = shared.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &state);
                });
            }
        });
        Self { url, state }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn models(&self) -> Vec<String> {
        self.state.lock().unwrap().models.clone()
    }

    /// `(method, path, body)` of each request.
    pub fn requests(&self) -> Vec<(String, String, Value)> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn respond(stream: &mut TcpStream, status: &str, kind: &str, body: &str) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(body.as_bytes())
}

fn chunked(stream: &mut TcpStream, kind: &str, parts: &[String]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
    );
    stream.write_all(head.as_bytes())?;
    for part in parts {
        stream.write_all(format!("{:x}\r\n{part}\r\n", part.len()).as_bytes())?;
        stream.flush()?;
        std::thread::sleep(Duration::from_millis(5));
    }
    stream.write_all(b"0\r\n\r\n")
}

fn handle(mut stream: TcpStream, state: &Mutex<State>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, path) = (
        parts.next().unwrap_or("").to_string(),
        parts.next().unwrap_or("").to_string(),
    );
    let mut length = 0;
    loop {
        let mut header = String::new();
        reader.read_line(&mut header)?;
        if header.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
    state
        .lock()
        .unwrap()
        .requests
        .push((method.clone(), path.clone(), body.clone()));
    let model = body["model"].as_str().unwrap_or("").to_string();
    match (method.as_str(), path.as_str()) {
        ("GET", "/api/tags") => {
            let models: Vec<Value> = state.lock().unwrap().models.iter().map(|name| json!({
                "name": name, "model": name, "size": 5_200_000_000u64, "modified_at": "2026-10-01T10:00:00Z",
                "details": {"family": "qwen3", "parameter_size": "8.2B", "quantization_level": "Q4_K_M"}
            })).collect();
            respond(
                &mut stream,
                "200 OK",
                "application/json",
                &json!({"models": models}).to_string(),
            )
        }
        ("POST", "/api/show") if state.lock().unwrap().models.contains(&model) => respond(
            &mut stream,
            "200 OK",
            "application/json",
            &show(&model).to_string(),
        ),
        ("POST", "/api/pull") if model == "missing:1" => chunked(
            &mut stream,
            "application/x-ndjson",
            &[
                json!({"status": "pulling manifest"}).to_string() + "\n",
                json!({"error": "pull model manifest: file does not exist"}).to_string() + "\n",
            ],
        ),
        ("POST", "/api/pull") => {
            let lines = [
                json!({"status": "pulling manifest"}),
                json!({"status": "pulling 6a0746a1ec1a", "digest": "sha256:6a07", "total": 4000, "completed": 1000}),
                json!({"status": "pulling 6a0746a1ec1a", "digest": "sha256:6a07", "total": 4000, "completed": 4000}),
                json!({"status": "verifying sha256 digest"}),
                json!({"status": "success"}),
            ];
            // Split one line across chunks to exercise the line buffer.
            let text: String = lines.iter().map(|l| l.to_string() + "\n").collect();
            let (a, b) = text.split_at(30);
            chunked(
                &mut stream,
                "application/x-ndjson",
                &[a.to_string(), b.to_string()],
            )?;
            state.lock().unwrap().models.push(model);
            Ok(())
        }
        ("DELETE", "/api/delete") => {
            state.lock().unwrap().models.retain(|m| *m != model);
            respond(&mut stream, "200 OK", "application/json", "{}")
        }
        ("POST", "/v1/chat/completions") => {
            let chunk = json!({"id": "c1", "object": "chat.completion.chunk", "created": 1, "model": model,
                "choices": [{"index": 0, "delta": {"role": "assistant", "content": "Hello from Ollama"}, "finish_reason": null}]});
            let done = json!({"id": "c1", "object": "chat.completion.chunk", "created": 1, "model": model,
                "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 3, "total_tokens": 13}});
            let events = [
                format!("data: {chunk}\n\n"),
                format!("data: {done}\n\n"),
                "data: [DONE]\n\n".to_string(),
            ];
            chunked(&mut stream, "text/event-stream", &events)
        }
        _ => respond(
            &mut stream,
            "404 Not Found",
            "application/json",
            &json!({"error": "not found"}).to_string(),
        ),
    }
}
