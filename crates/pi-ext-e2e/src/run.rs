//! One `pi --mode json -p <prompt>` run with a native extension package and the mock provider.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::{MockProvider, Output, TempDir, Turn, pi_bin};

const PROVIDER: &str = "mock";
const MODEL: &str = "mock-model";

/// Builder for one run. `run()` returns the JSON events, the provider requests, and stderr.
pub struct Run {
    binary: PathBuf,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    turns: Vec<Turn>,
    prompt: String,
    settings: Value,
    pi_args: Vec<String>,
    timeout: Duration,
    pi: PathBuf,
    files: Vec<(PathBuf, String)>,
}

impl Run {
    /// A run whose package lists `binary` in `pi.native`.
    pub fn new(binary: impl Into<PathBuf>) -> Self {
        Self {
            binary: binary.into(),
            args: Vec::new(),
            env: BTreeMap::from([("PI_EXT_LOG".into(), "1".into())]),
            turns: Vec::new(),
            prompt: "hello".into(),
            settings: json!({}),
            pi_args: Vec::new(),
            timeout: Duration::from_secs(60),
            pi: pi_bin(),
            files: Vec::new(),
        }
    }

    /// `args` of the `pi.native` entry, for example `["todo"]` for `pi-rust-ext`.
    pub fn args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// `env` of the `pi.native` entry.
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.insert(key.into(), value.into());
        self
    }

    pub fn turns(mut self, turns: impl IntoIterator<Item = Turn>) -> Self {
        self.turns = turns.into_iter().collect();
        self
    }

    pub fn prompt(mut self, prompt: impl Into<String>) -> Self {
        self.prompt = prompt.into();
        self
    }

    /// Keys merged into the global `settings.json`.
    pub fn settings(mut self, settings: Value) -> Self {
        self.settings = settings;
        self
    }

    /// Extra pi arguments before `-p`, for example `["--native-flag", "on"]`.
    pub fn pi_args<S: Into<String>>(mut self, args: impl IntoIterator<Item = S>) -> Self {
        self.pi_args = args.into_iter().map(Into::into).collect();
        self
    }

    /// A file written before pi starts. `path` is relative to the run root: `work/...` is in
    /// the cwd and `agent/...` in the agent dir. Files are written last and replace generated ones.
    pub fn file(mut self, path: impl Into<PathBuf>, content: impl Into<String>) -> Self {
        self.files.push((path.into(), content.into()));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The pi binary in place of `PI_BIN`.
    pub fn pi(mut self, pi: impl Into<PathBuf>) -> Self {
        self.pi = pi.into();
        self
    }

    pub fn run(self) -> Result<Output, String> {
        let provider =
            MockProvider::start(self.turns.clone()).map_err(|error| error.to_string())?;
        let root = TempDir::new().map_err(|error| error.to_string())?;
        self.write_files(root.path(), provider.url())
            .map_err(|error| error.to_string())?;
        let mut child = Command::new(&self.pi)
            .args(["--mode", "json"])
            .args(&self.pi_args)
            .args(["-p", &self.prompt])
            .current_dir(root.path().join("work"))
            .env_clear()
            .envs(base_env(root.path()))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("cannot start {}: {error}", self.pi.display()))?;
        let stdout = reader(child.stdout.take());
        let stderr = reader(child.stderr.take());
        let deadline = Instant::now() + self.timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(|error| error.to_string())? {
                break status;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                let stderr = stderr.join().unwrap_or_default();
                return Err(format!(
                    "pi did not exit within {:?}; stderr:\n{stderr}",
                    self.timeout
                ));
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let stdout = stdout.join().unwrap_or_default();
        let stderr = stderr.join().unwrap_or_default();
        let events = stdout
            .lines()
            .filter_map(|line| serde_json::from_str(line).ok())
            .collect();
        Ok(Output {
            status,
            events,
            requests: provider.requests(),
            stdout,
            stderr,
        })
    }

    fn write_files(&self, root: &Path, url: &str) -> std::io::Result<()> {
        for dir in ["agent", "work", "pkg"] {
            std::fs::create_dir_all(root.join(dir))?;
        }
        let models = json!({"providers": {PROVIDER: {"baseUrl": url, "api": "openai-completions", "apiKey": "e2e-key",
            "models": [{"id": MODEL, "contextWindow": 100000, "maxTokens": 4000}]}}});
        std::fs::write(root.join("agent/models.json"), models.to_string())?;
        let native = json!({"command": self.binary, "args": self.args, "env": self.env});
        let package = json!({"name": "e2e", "pi": {"native": [native]}});
        std::fs::write(root.join("pkg/package.json"), package.to_string())?;
        let mut settings = json!({"defaultProvider": PROVIDER, "defaultModel": MODEL,
            "packages": [root.join("pkg").display().to_string()]});
        if let (Some(target), Some(extra)) = (settings.as_object_mut(), self.settings.as_object()) {
            target.extend(extra.clone());
        }
        std::fs::write(root.join("agent/settings.json"), settings.to_string())?;
        for (path, content) in &self.files {
            let path = root.join(checked(path)?);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, content)?;
        }
        Ok(())
    }
}

/// `path` when it starts with `work` or `agent` and stays inside the run root.
fn checked(path: &Path) -> std::io::Result<&Path> {
    let mut parts = path.components();
    let root = parts.next().and_then(|part| part.as_os_str().to_str());
    let inside = parts.all(|part| matches!(part, Component::Normal(_)));
    if inside && matches!(root, Some("work" | "agent")) && path.components().count() > 1 {
        return Ok(path);
    }
    let message = format!(
        "Run::file path {} must be under work/ or agent/",
        path.display()
    );
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        message,
    ))
}

fn base_env(root: &Path) -> Vec<(String, String)> {
    let mut env = vec![
        ("HOME".into(), root.display().to_string()),
        (
            "PI_CODING_AGENT_DIR".into(),
            root.join("agent").display().to_string(),
        ),
        ("PI_OFFLINE".into(), "1".into()),
    ];
    for key in ["PATH", "TMPDIR", "LANG", "SYSTEMROOT"] {
        if let Ok(value) = std::env::var(key) {
            env.push((key.into(), value));
        }
    }
    env
}

fn reader(stream: Option<impl Read + Send + 'static>) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut stream) = stream {
            let _ = stream.read_to_string(&mut text);
        }
        text
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_paths_stay_in_work_or_agent() {
        for ok in [
            "work/AGENTS.md",
            "agent/extensions/a.json",
            "work/.pi/settings.json",
        ] {
            assert!(checked(Path::new(ok)).is_ok(), "{ok}");
        }
        for bad in [
            "/etc/passwd",
            "work",
            "pkg/package.json",
            "work/../x",
            "agent/../../x",
            "x/y",
        ] {
            assert!(checked(Path::new(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn files_are_written_after_generated_files() {
        let root = TempDir::new().unwrap();
        let run = Run::new("/bin/true")
            .file("work/notes/a.txt", "a")
            .file("agent/settings.json", "{}");
        run.write_files(root.path(), "http://127.0.0.1:1").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.path().join("work/notes/a.txt")).unwrap(),
            "a"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("agent/settings.json")).unwrap(),
            "{}"
        );
        let bad = Run::new("/bin/true").file("../escape", "x");
        assert!(bad.write_files(root.path(), "http://127.0.0.1:1").is_err());
    }
}
