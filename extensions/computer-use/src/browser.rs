use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use pi_ext_sdk::{Error, Result, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::cdp;
use crate::config::ManagedBrowser;
use crate::live::Live;
use crate::output;
use crate::page;
use crate::platform::run::which;
use crate::query::require_state;
use crate::runtime::state_dir;
use crate::state::{Observation, Target};

pub struct Managed {
    pub port: u16,
    pub name: &'static str,
    pub pid: i64,
    pub child: Option<Child>,
}

impl Drop for Managed {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BrowserArgs {
    pub url: Option<String>,
    pub state_id: String,
    pub expression: String,
}

pub fn display_name(browser: ManagedBrowser) -> &'static str {
    match browser {
        ManagedBrowser::Chrome => "Chrome",
        ManagedBrowser::Helium => "Helium",
    }
}

pub fn executable(
    browser: ManagedBrowser,
    lookup: impl Fn(&str) -> Option<String>,
    has: impl Fn(&str) -> bool,
) -> Option<PathBuf> {
    let (env, mac, linux): (&str, &[&str], &[&str]) = match browser {
        ManagedBrowser::Chrome => (
            "PI_COMPUTER_USE_CHROME_EXECUTABLE",
            &[
                "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
                "/Applications/Chromium.app/Contents/MacOS/Chromium",
            ],
            &[
                "google-chrome",
                "google-chrome-stable",
                "chromium",
                "chromium-browser",
            ],
        ),
        ManagedBrowser::Helium => (
            "PI_COMPUTER_USE_HELIUM_EXECUTABLE",
            &["/Applications/Helium.app/Contents/MacOS/Helium"],
            &["helium", "helium-browser"],
        ),
    };
    if let Some(path) = lookup(env).filter(|p| !p.trim().is_empty()) {
        return Some(PathBuf::from(path));
    }
    if cfg!(target_os = "macos") {
        return mac.iter().map(PathBuf::from).find(|p| p.is_file());
    }
    linux.iter().find(|name| has(name)).map(PathBuf::from)
}

pub fn launch_args(port: u16, profile: &Path) -> Vec<String> {
    vec![
        format!("--remote-debugging-port={port}"),
        format!("--user-data-dir={}", profile.display()),
        "--no-first-run".to_owned(),
        "--no-default-browser-check".to_owned(),
        "about:blank".to_owned(),
    ]
}

/// HTTP(S) only; `about:blank` is accepted for an empty page.
pub fn check_url(url: &str) -> Result<()> {
    let lower = url.trim().to_ascii_lowercase();
    if lower == "about:blank" || lower.starts_with("http://") || lower.starts_with("https://") {
        return Ok(());
    }
    Err(Error::msg(format!(
        "Browser navigation requires an HTTP(S) URL, not '{url}'."
    )))
}

pub fn pick_port(lookup: impl Fn(&str) -> Option<String>) -> Result<u16> {
    if let Some(port) = lookup("PI_COMPUTER_USE_CDP_PORT").and_then(|p| p.trim().parse().ok()) {
        return Ok(port);
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

async fn wait_ready(port: u16, child: &mut Child) -> Result<()> {
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(20) {
        if cdp::get_json(port, "/json/version").await.is_ok() {
            return Ok(());
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(Error::msg(format!(
                "The managed browser exited during start ({status})."
            )));
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err(Error::msg(format!(
        "The managed browser did not open DevTools on port {port} within 20s."
    )))
}

impl Live<'_> {
    async fn ensure_browser(&self) -> Result<u16> {
        if let Some(port) = page::port(self.rt) {
            if cdp::get_json(port, "/json/version").await.is_ok() {
                return Ok(port);
            }
            *self.rt.browser() = None;
        }
        let env = |key: &str| std::env::var(key).ok();
        let browser = self.rt.config().managed_browser;
        let name = display_name(browser);
        let port = pick_port(env)?;
        if cdp::get_json(port, "/json/version").await.is_ok() {
            *self.rt.browser() = Some(Managed {
                port,
                name,
                pid: 0,
                child: None,
            });
            return Ok(port);
        }
        let binary = executable(browser, env, which).ok_or_else(|| {
            Error::msg(format!(
                "{name} was not found. Install it or set PI_COMPUTER_USE_{}_EXECUTABLE.",
                name.to_uppercase()
            ))
        })?;
        let profile = state_dir(self.host).join(format!("browser-{}", browser.as_str()));
        std::fs::create_dir_all(&profile)?;
        let mut child = Command::new(&binary)
            .args(launch_args(port, &profile))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|err| Error::msg(format!("Could not start {}: {err}", binary.display())))?;
        if let Err(err) = wait_ready(port, &mut child).await {
            let _ = child.kill();
            return Err(err);
        }
        let pid = i64::from(child.id());
        *self.rt.browser() = Some(Managed {
            port,
            name,
            pid,
            child: Some(child),
        });
        Ok(port)
    }

    async fn browser_state(&self, state_id: &str) -> Result<Observation> {
        let base = require_state(&self.rt.store(), state_id)?;
        if base.target.page_id.is_none() {
            return Err(Error::msg(format!(
                "State '{state_id}' is a desktop state. Use act_ui for native windows; this tool is CDP-only."
            )));
        }
        Ok(base)
    }

    async fn page_result(
        &self,
        tool: &str,
        summary: &str,
        target: &Target,
        base: Option<&str>,
    ) -> Result<ToolResult> {
        let (observation, image) = self.capture(target, "fused").await?;
        Ok(output::observation_result(
            &mut self.rt.store(),
            tool,
            summary,
            &observation,
            image,
            base,
        ))
    }

    pub async fn launch_browser(&self, args: BrowserArgs) -> Result<ToolResult> {
        if !self.rt.config().browser_use {
            return Err(Error::msg(crate::live::browser_refused("launch_browser")));
        }
        let url = args.url.as_deref().map(str::trim).filter(|u| !u.is_empty());
        if let Some(url) = url {
            check_url(url)?;
        }
        let port = self.ensure_browser().await?;
        let roots = page::roots(self.rt).await?;
        let root = roots
            .into_iter()
            .next()
            .ok_or_else(|| Error::msg("The managed browser has no page target."))?;
        let root = self.rt.store().put_root(root);
        let target = Target::from_root(&root, Some(port));
        if let Some(url) = url {
            page::navigate(&target, url, self.cancel).await?;
        }
        let summary = format!("Launched the managed browser on DevTools port {port}.");
        self.page_result("launch_browser", &summary, &target, None)
            .await
    }

    pub async fn navigate_browser(&self, args: BrowserArgs) -> Result<ToolResult> {
        let base = self.browser_state(&args.state_id).await?;
        let url = args.url.unwrap_or_default();
        check_url(&url)?;
        crate::tools::check_current(self, &base)?;
        page::navigate(&base.target, url.trim(), self.cancel).await?;
        let summary = format!("Navigated to {}.", url.trim());
        self.page_result(
            "navigate_browser",
            &summary,
            &base.target,
            Some(&base.capture.state_id),
        )
        .await
    }

    pub async fn evaluate_browser(&self, args: BrowserArgs) -> Result<ToolResult> {
        let base = self.browser_state(&args.state_id).await?;
        if args.expression.trim().is_empty() || args.expression.len() > 65_536 {
            return Err(Error::msg("expression must be 1 to 65536 bytes."));
        }
        crate::tools::check_current(self, &base)?;
        let value = page::open(&base.target, self.cancel)
            .await?
            .evaluate(&args.expression)
            .await?;
        let text = output::bound(&mut self.rt.store(), format!("Evaluation value: {value}"));
        let mut result = self
            .page_result(
                "evaluate_browser",
                "Evaluated the expression.",
                &base.target,
                Some(&base.capture.state_id),
            )
            .await?;
        result.content.push(pi_ext_sdk::content::text(text));
        if let Some(details) = result.details.as_mut() {
            details["baseStateId"] = json!(base.capture.state_id);
        }
        Ok(result)
    }
}
