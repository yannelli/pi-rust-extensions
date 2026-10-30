use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;

const MARKER: &str = "launched.json";

pub async fn discover(host: &str, port: u16) -> Result<(String, Value)> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(Duration::from_secs(2))
        .build()?;
    let url = format!("http://{}/json/version", host_port(host, port));
    let version: Value = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("no DevTools endpoint at {url}"))?
        .error_for_status()?
        .json()
        .await
        .with_context(|| format!("{url} did not return JSON"))?;
    let ws = version
        .get("webSocketDebuggerUrl")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("{url} has no webSocketDebuggerUrl"))?;
    Ok((ws.to_owned(), version))
}

fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

pub fn launch_args(config: &Config, url: Option<&str>) -> Vec<String> {
    let mut args = vec![
        format!("--remote-debugging-port={}", config.port),
        format!("--user-data-dir={}", config.user_data_dir.display()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
    ];
    if config.headless {
        args.push("--headless=new".to_owned());
    }
    args.extend(config.extra_args.iter().cloned());
    args.push(url.unwrap_or("about:blank").to_owned());
    args
}

pub fn chrome_binary(config: &Config) -> Result<PathBuf> {
    if let Some(path) = &config.chrome_path {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("CHROME_PATH").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
            "/Applications/Google Chrome Canary.app/Contents/MacOS/Google Chrome Canary",
        ]
    } else if cfg!(windows) {
        &[
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ]
    } else {
        &[
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/snap/bin/chromium",
        ]
    };
    candidates
        .iter()
        .map(PathBuf::from)
        .find(|path| path.exists())
        .ok_or_else(|| anyhow!("Chrome not found. Set chromePath in /chrome config."))
}

/// The browser WebSocket URL holds a per-process id, so a reused port cannot match it.
#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct LaunchMarker {
    pub browser_ws: String,
    pub pid: Option<u32>,
}

pub async fn launch(config: &Config, state_dir: &Path, url: Option<&str>) -> Result<String> {
    let binary = chrome_binary(config)?;
    std::fs::create_dir_all(&config.user_data_dir)
        .with_context(|| format!("{}", config.user_data_dir.display()))?;
    let child = tokio::process::Command::new(&binary)
        .args(launch_args(config, url))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false)
        .spawn()
        .with_context(|| format!("cannot start {}", binary.display()))?;
    let pid = child.id();
    let wait = async {
        loop {
            if let Ok((ws, _)) = discover(&config.host, config.port).await {
                return ws;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    };
    match tokio::time::timeout(Duration::from_secs(15), wait).await {
        Ok(browser_ws) => {
            let marker = LaunchMarker {
                browser_ws: browser_ws.clone(),
                pid,
            };
            write_marker(state_dir, &marker);
            Ok(browser_ws)
        }
        Err(_) => bail!(
            "Chrome started but port {} did not answer in 15 seconds. Another Chrome can use the same profile.",
            config.port
        ),
    }
}

fn write_marker(state_dir: &Path, marker: &LaunchMarker) {
    let write = std::fs::create_dir_all(state_dir).and_then(|()| {
        let text = serde_json::to_string(marker).unwrap_or_default();
        std::fs::write(state_dir.join(MARKER), text)
    });
    if let Err(err) = write {
        pi_ext_sdk::warn!("cannot write the launch marker: {err}");
    }
}

/// True when the marker names the browser at `browser_ws`; `take` removes a matching marker.
pub fn launched_here(state_dir: &Path, browser_ws: &str, take: bool) -> bool {
    let path = state_dir.join(MARKER);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return false;
    };
    let ours =
        serde_json::from_str::<LaunchMarker>(&text).is_ok_and(|m| m.browser_ws == browser_ws);
    if ours && take {
        let _ = std::fs::remove_file(path);
    }
    ours
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigFile;

    #[test]
    fn launch_args_use_profile_and_port() {
        let file = ConfigFile {
            port: Some(9333),
            headless: Some(true),
            extra_args: Some(vec!["--lang=en".to_owned()]),
            ..ConfigFile::default()
        };
        let config = Config::resolve(file, Path::new("/s"));
        let args = launch_args(&config, Some("https://example.com"));
        assert_eq!(args[0], "--remote-debugging-port=9333");
        assert_eq!(args[1], "--user-data-dir=/s/profile");
        assert!(args.contains(&"--headless=new".to_owned()));
        assert_eq!(args.last().unwrap(), "https://example.com");
    }

    #[test]
    fn marker_matches_port() {
        let dir = tempfile::tempdir().unwrap();
        let ws = "ws://127.0.0.1:9222/devtools/browser/a1";
        let marker = LaunchMarker {
            browser_ws: ws.to_owned(),
            pid: None,
        };
        write_marker(dir.path(), &marker);
        assert!(!launched_here(
            dir.path(),
            "ws://127.0.0.1:9222/devtools/browser/b2",
            true
        ));
        assert!(launched_here(dir.path(), ws, true));
        assert!(!launched_here(dir.path(), ws, false));
    }

    #[test]
    fn formats_ipv6_hosts() {
        assert_eq!(host_port("::1", 9222), "[::1]:9222");
        assert_eq!(host_port("127.0.0.1", 9222), "127.0.0.1:9222");
    }
}
