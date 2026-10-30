use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{Result, anyhow};
use pi_ext_sdk::Host;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::auth::Grant;
use crate::browser;
use crate::cdp::Cdp;
use crate::config::{self, Config, ConfigFile, Mode};
use crate::page::Page;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TargetSel {
    pub target_id: Option<String>,
    pub url_includes: Option<String>,
    pub title_includes: Option<String>,
    pub background: Option<bool>,
}

impl TargetSel {
    pub fn is_empty(&self) -> bool {
        self.target_id.is_none() && self.url_includes.is_none() && self.title_includes.is_none()
    }

    pub fn matches(&self, target: &Value) -> bool {
        let field = |key: &str| target.get(key).and_then(Value::as_str).unwrap_or("");
        self.target_id
            .as_deref()
            .is_none_or(|id| field("targetId") == id)
            && self
                .url_includes
                .as_deref()
                .is_none_or(|u| field("url").contains(u))
            && self
                .title_includes
                .as_deref()
                .is_none_or(|t| field("title").contains(t))
    }
}

#[derive(Default)]
pub struct Chrome {
    config: Mutex<Option<Config>>,
    pub grant: Mutex<Grant>,
    pub generation: AtomicU64,
    background: Mutex<Option<bool>>,
    cdp: tokio::sync::Mutex<Option<Arc<Cdp>>>,
    browser_ws: Mutex<Option<String>>,
    owned: Mutex<Vec<String>>,
    pub snapshots: Mutex<HashMap<String, Vec<String>>>,
    trust_notice: AtomicBool,
}

impl Chrome {
    pub fn state_dir(host: &Host) -> PathBuf {
        config::state_dir(&host.agent_dir())
    }

    /// `isProjectTrusted` from the ctx snapshot, else `ctx.isProjectTrusted()`. Fails closed.
    pub async fn trusted(host: &Host) -> bool {
        let from_snapshot = host
            .snapshot()
            .and_then(|s| s.extra.get("isProjectTrusted").and_then(Value::as_bool));
        match from_snapshot {
            Some(trusted) => trusted,
            None => host
                .pi_call("ctx", "isProjectTrusted", vec![])
                .await
                .ok()
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        }
    }

    pub async fn reload_config(&self, host: &Host) -> Result<Config> {
        let global = config::read_file(&config::global_path(&host.agent_dir()))?;
        let project_path = config::project_path(&host.cwd());
        let project = if !project_path.exists() {
            ConfigFile::default()
        } else if Self::trusted(host).await {
            config::read_file(&project_path)?
        } else {
            if !self.trust_notice.swap(true, Ordering::SeqCst) {
                let text = format!(
                    "chrome: ignoring {} because the project is not trusted.",
                    project_path.display()
                );
                let _ = host.notify_warning(text).await;
            }
            ConfigFile::default()
        };
        let resolved = Config::resolve(ConfigFile::merge(global, project), &Self::state_dir(host));
        *self.config.lock().expect("config") = Some(resolved.clone());
        Ok(resolved)
    }

    pub async fn config(&self, host: &Host) -> Result<Config> {
        let cached = self.config.lock().expect("config").clone();
        match cached {
            Some(config) => Ok(config),
            None => self.reload_config(host).await,
        }
    }

    pub fn authorized(&self) -> bool {
        self.grant
            .lock()
            .expect("grant")
            .is_active(SystemTime::now())
    }

    pub async fn background(&self, host: &Host) -> bool {
        let session = *self.background.lock().expect("background");
        match session {
            Some(value) => value,
            None => self
                .config(host)
                .await
                .map(|c| c.background)
                .unwrap_or(true),
        }
    }

    pub fn set_background(&self, value: Option<bool>) {
        *self.background.lock().expect("background") = value;
    }

    pub fn set_config(&self, config: Config) {
        *self.config.lock().expect("config") = Some(config);
    }

    pub async fn cdp(&self, host: &Host) -> Result<Arc<Cdp>> {
        let launch = self.config(host).await?.mode != Mode::Connect;
        self.ensure(host, launch).await
    }

    /// Connects to the DevTools port; when nothing answers, starts the managed profile if `launch`.
    pub async fn ensure(&self, host: &Host, launch: bool) -> Result<Arc<Cdp>> {
        let mut slot = self.cdp.lock().await;
        if let Some(cdp) = slot.as_ref().filter(|c| !c.is_closed()) {
            return Ok(cdp.clone());
        }
        let config = self.config(host).await?;
        let ws = match browser::discover(&config.host, config.port).await {
            Ok((ws, _)) => ws,
            Err(err) if !launch => {
                return Err(err.context(format!(
                    "Start Chrome with --remote-debugging-port={} or set mode to auto in /chrome config",
                    config.port
                )));
            }
            Err(_) => browser::launch(&config, &Self::state_dir(host), None).await?,
        };
        let cdp = Arc::new(Cdp::connect(&ws).await?);
        *slot = Some(cdp.clone());
        *self.browser_ws.lock().expect("browser_ws") = Some(ws);
        Ok(cdp)
    }

    pub async fn connected(&self) -> Option<Arc<Cdp>> {
        self.cdp.lock().await.clone().filter(|c| !c.is_closed())
    }

    pub async fn browser_call(&self, host: &Host, method: &str, params: Value) -> Result<Value> {
        let cdp = self.cdp(host).await?;
        cdp.call(method, params, None, DEFAULT_TIMEOUT).await
    }

    pub async fn pages(&self, host: &Host) -> Result<Vec<Value>> {
        let reply = self
            .browser_call(host, "Target.getTargets", json!({}))
            .await?;
        let infos = reply["targetInfos"].as_array().cloned().unwrap_or_default();
        Ok(infos.into_iter().filter(|t| t["type"] == "page").collect())
    }

    pub fn owned(&self) -> Vec<String> {
        self.owned.lock().expect("owned").clone()
    }

    pub fn own(&self, target_id: &str) {
        self.owned.lock().expect("owned").push(target_id.to_owned());
    }

    pub fn disown(&self, target_id: &str) {
        self.owned
            .lock()
            .expect("owned")
            .retain(|id| id != target_id);
    }

    pub async fn new_tab(&self, host: &Host, url: &str) -> Result<String> {
        let background = self.background(host).await;
        let params = json!({ "url": url, "background": background });
        let reply = self
            .browser_call(host, "Target.createTarget", params)
            .await?;
        let id = reply["targetId"]
            .as_str()
            .ok_or_else(|| anyhow!("no targetId"))?;
        self.own(id);
        Ok(id.to_owned())
    }

    /// The tab a selector names, else this session's own tab (created on first use).
    pub async fn resolve(&self, host: &Host, sel: &TargetSel) -> Result<String> {
        let pages = self.pages(host).await?;
        if !sel.is_empty() {
            return pages
                .iter()
                .find(|page| sel.matches(page))
                .and_then(|page| page["targetId"].as_str().map(str::to_owned))
                .ok_or_else(|| {
                    anyhow!("No Chrome tab matches {sel:?}. Run chrome_tab with action list.")
                });
        }
        let live = |id: &String| pages.iter().any(|p| p["targetId"] == id.as_str());
        let owned = self.owned();
        if let Some(id) = owned.iter().rev().find(|id| live(id)) {
            return Ok(id.clone());
        }
        self.owned.lock().expect("owned").retain(|id| live(id));
        self.new_tab(host, "about:blank").await
    }

    pub async fn page(&self, host: &Host, sel: &TargetSel) -> Result<Page> {
        let target_id = self.resolve(host, sel).await?;
        let cdp = self.cdp(host).await?;
        let params = json!({ "targetId": target_id, "flatten": true });
        let reply = cdp
            .call("Target.attachToTarget", params, None, DEFAULT_TIMEOUT)
            .await?;
        let session = reply["sessionId"]
            .as_str()
            .ok_or_else(|| anyhow!("no sessionId"))?;
        let page = Page::new(cdp, session.to_owned(), target_id);
        if sel.background == Some(false) && !self.background(host).await {
            page.call("Page.bringToFront", json!({})).await?;
        }
        Ok(page)
    }

    /// Closes the tabs this session opened. Errors are ignored.
    pub async fn close_owned(&self) {
        let Some(cdp) = self.connected().await else {
            return;
        };
        let owned = std::mem::take(&mut *self.owned.lock().expect("owned"));
        for id in owned {
            let params = json!({ "targetId": id });
            let _ = cdp
                .call("Target.closeTarget", params, None, Duration::from_secs(1))
                .await;
        }
    }

    pub fn browser_ws(&self) -> Option<String> {
        self.browser_ws.lock().expect("browser_ws").clone()
    }

    pub async fn disconnect(&self) {
        self.cdp.lock().await.take();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_matches_fields() {
        let page = json!({ "targetId": "T1", "url": "https://a.dev/x", "title": "Alpha" });
        let sel = |id: Option<&str>, url: Option<&str>, title: Option<&str>| TargetSel {
            target_id: id.map(str::to_owned),
            url_includes: url.map(str::to_owned),
            title_includes: title.map(str::to_owned),
            background: None,
        };
        assert!(sel(Some("T1"), None, None).matches(&page));
        assert!(sel(None, Some("a.dev"), Some("Alp")).matches(&page));
        assert!(!sel(None, Some("b.dev"), None).matches(&page));
        assert!(sel(None, None, None).is_empty());
    }
}
