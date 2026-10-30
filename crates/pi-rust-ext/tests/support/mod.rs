use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use pi_ext_e2e::Value;

pub struct LocalApi {
    pub url: String,
    pub requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl LocalApi {
    pub fn start(response: Value) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stopped = stop.clone();
        let worker = std::thread::spawn(move || {
            while !stopped.load(std::sync::atomic::Ordering::Relaxed) {
                let (mut stream, _) = match listener.accept() {
                    Ok(connection) => connection,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                    Err(error) => panic!("local API accept: {error}"),
                };
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                let mut length = 0;
                loop {
                    line.clear();
                    if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        length = value.trim().parse().unwrap();
                    }
                }
                let mut body = vec![0; length];
                reader.read_exact(&mut body).unwrap();
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(body).unwrap().parse().unwrap());
                let body = response.to_string();
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                stream.flush().unwrap();
            }
        });
        Self {
            url,
            requests,
            stop,
            worker: Some(worker),
        }
    }
}

impl Drop for LocalApi {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("local API server exits");
        }
    }
}

pub fn configured_search(config: Value) -> (pi_ext_e2e::TempDir, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = pi_ext_e2e::TempDir::new().unwrap();
    let config_path = dir.path().join("search.json");
    std::fs::write(&config_path, config.to_string()).unwrap();
    let wrapper = dir.path().join("search-wrapper");
    let quote = |s: &str| format!("'{}'", s.replace('\'', "'\\''"));
    let script = format!(
        "#!/bin/sh\nset -eu\nmkdir -p \"$PI_CODING_AGENT_DIR/ext\"\ncp {} \"$PI_CODING_AGENT_DIR/ext/web-search.json\"\nexec {} \"$@\"\n",
        quote(config_path.to_str().unwrap()),
        quote(env!("CARGO_BIN_EXE_pi-rust-ext")),
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    (dir, wrapper)
}
