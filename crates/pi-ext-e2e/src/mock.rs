//! A local OpenAI-compatible chat-completions server that streams scripted turns.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::Turn;

#[derive(Default)]
struct Script {
    turns: Vec<Turn>,
    served: usize,
    requests: Vec<Value>,
}

/// Serves `POST <url>/chat/completions` on 127.0.0.1 until dropped. Each request takes the
/// next turn; after the last one it answers with an empty text turn.
pub struct MockProvider {
    url: String,
    script: Arc<Mutex<Script>>,
}

impl MockProvider {
    pub fn start(turns: Vec<Turn>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/v1", listener.local_addr()?);
        let script = Arc::new(Mutex::new(Script {
            turns,
            ..Script::default()
        }));
        let shared = script.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let script = shared.clone();
                std::thread::spawn(move || {
                    let _ = serve(stream, &script);
                });
            }
        });
        Ok(Self { url, script })
    }

    /// The `baseUrl` for `models.json`.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Request bodies received so far, in order.
    pub fn requests(&self) -> Vec<Value> {
        self.script.lock().expect("script lock").requests.clone()
    }
}

fn serve(stream: TcpStream, script: &Mutex<Script>) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut length = 0;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse().unwrap_or(0);
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    let request = serde_json::from_slice(&body).unwrap_or(Value::Null);
    let chunks = {
        let mut script = script.lock().expect("script lock");
        script.requests.push(request);
        let index = script.served;
        script.served += 1;
        script
            .turns
            .get(index)
            .cloned()
            .unwrap_or_else(|| Turn::text(""))
            .chunks(index)
    };
    let mut payload: String = chunks
        .iter()
        .map(|chunk| format!("data: {chunk}\n\n"))
        .collect();
    payload.push_str("data: [DONE]\n\n");
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        payload.len()
    );
    let mut stream = stream;
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()
}
