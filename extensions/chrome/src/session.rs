use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime};

use pi_ext_sdk::style::{self, Color};
use pi_ext_sdk::{Host, Result};

use crate::auth::{ENTRY_TYPE, Grant, ToolChange, epoch_ms};
use crate::browser;
use crate::specs::TOOL_NAMES;
use crate::state::Chrome;

pub const PRIMER_KEY: &str = "chrome-profile-bridge";
pub const PRIMER: &str = "Chrome control is authorized for this session through the chrome_* tools. \
Use chrome_snapshot or chrome_find to read the page and get element uids, then act with chrome_click, \
chrome_type, chrome_fill, chrome_key, chrome_scroll, and chrome_navigate. With no tab selector, the tools \
use this session's own tab. Do not change the user's other tabs unless the user asks.";

pub async fn apply_tools(host: &Host, authorized: bool) -> Result<()> {
    let active = host.active_tools().await?;
    let mut next: Vec<String> = active
        .into_iter()
        .filter(|name| !TOOL_NAMES.contains(&name.as_str()))
        .collect();
    if authorized {
        next.extend(TOOL_NAMES.iter().map(|name| (*name).to_owned()));
    }
    host.set_active_tools(&next).await
}

pub async fn update_status(host: &Host, chrome: &Chrome) {
    let grant = *chrome.grant.lock().expect("grant");
    let text = grant.is_active(SystemTime::now()).then(|| {
        let dot = style::fg(Color::Green, "●");
        format!("{dot} Chrome · {}", grant.describe(SystemTime::now()))
    });
    if host.has_ui() {
        let _ = host.set_status("chrome", text.as_deref()).await;
    }
}

async fn record(host: &Host, grant: Grant, action: &str) {
    let content = match grant.remaining(SystemTime::now()) {
        Some(left) => format!(
            "[pi-chrome] Chrome tools {action} for {}.",
            crate::auth::countdown(left)
        ),
        None => format!("[pi-chrome] Chrome tools {action}."),
    };
    let change = ToolChange {
        content,
        action: action.to_owned(),
        tools: TOOL_NAMES.iter().map(|name| (*name).to_owned()).collect(),
        authorized_until: grant.until_ms(),
        indefinite: grant == Grant::Indefinite,
        session_id: host.snapshot().and_then(|s| s.session_id.clone()),
        at: epoch_ms(SystemTime::now()),
    };
    if let Err(err) = host.append_entry(ENTRY_TYPE, &change).await {
        pi_ext_sdk::warn!("cannot record the chrome tool change: {}", err.message());
    }
}

/// Activates the tools under `grant`. `action` is `authorized`, `reauthorized`, or empty for a restore.
pub async fn grant(chrome: &Arc<Chrome>, host: &Host, grant: Grant, action: &str) -> Result<()> {
    *chrome.grant.lock().expect("grant") = grant;
    let generation = chrome.generation.fetch_add(1, Ordering::SeqCst) + 1;
    apply_tools(host, true).await?;
    if !action.is_empty() {
        record(host, grant, action).await;
    }
    update_status(host, chrome).await;
    if matches!(grant, Grant::Until(_)) {
        tokio::spawn(expiry_timer(chrome.clone(), host.clone(), generation));
    }
    Ok(())
}

/// Locks the tools and closes this session's tabs. `action` is `revoked` or `expired`.
pub async fn lock(chrome: &Chrome, host: &Host, action: &str) -> Result<()> {
    let previous = std::mem::take(&mut *chrome.grant.lock().expect("grant"));
    chrome.generation.fetch_add(1, Ordering::SeqCst);
    apply_tools(host, false).await?;
    if previous != Grant::Locked {
        record(host, Grant::Locked, action).await;
    }
    update_status(host, chrome).await;
    chrome.close_owned().await;
    Ok(())
}

async fn expiry_timer(chrome: Arc<Chrome>, host: Host, generation: u64) {
    loop {
        let grant = *chrome.grant.lock().expect("grant");
        let left = grant.remaining(SystemTime::now()).unwrap_or_default();
        tokio::time::sleep(left.min(Duration::from_secs(60))).await;
        if chrome.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if !chrome.authorized() {
            let _ = lock(&chrome, &host, "expired").await;
            let _ = host
                .notify_info("Chrome control expired. Run /chrome authorize to allow it again.")
                .await;
            return;
        }
        update_status(&host, &chrome).await;
    }
}

/// `session_start`: a `/reload` restores a valid grant of the same session. Other reasons reset all
/// session state, so an old expiry timer or tab cannot act in the new session.
pub async fn start(chrome: &Arc<Chrome>, host: &Host, reason: &str) -> Result<()> {
    let _ = chrome.reload_config(host).await;
    if reason == "reload" {
        let changes = host
            .entries_of::<ToolChange>(ENTRY_TYPE)
            .await
            .unwrap_or_default();
        let session_id = host.snapshot().and_then(|s| s.session_id.clone());
        let restored = changes
            .last()
            .map(|c| c.restorable(session_id.as_deref(), SystemTime::now()))
            .unwrap_or_default();
        if restored != Grant::Locked {
            return grant(chrome, host, restored, "").await;
        }
    }
    *chrome.grant.lock().expect("grant") = Grant::Locked;
    chrome.generation.fetch_add(1, Ordering::SeqCst);
    chrome.set_background(None);
    chrome.snapshots.lock().expect("snapshots").clear();
    let _ = tokio::time::timeout(Duration::from_millis(1_500), chrome.close_owned()).await;
    apply_tools(host, false).await?;
    update_status(host, chrome).await;
    Ok(())
}

/// `session_shutdown`: closes this session's tabs, and the managed Chrome when this process started it.
pub async fn shutdown(chrome: &Chrome, host: &Host, reason: &str) {
    if reason == "reload" {
        return;
    }
    chrome.close_owned().await;
    let Some(cdp) = chrome.connected().await else {
        return;
    };
    let close = chrome.config(host).await.is_ok_and(|c| c.close_on_exit);
    let ws = chrome.browser_ws().unwrap_or_default();
    if close && browser::launched_here(&Chrome::state_dir(host), &ws, true) {
        let params = serde_json::json!({});
        let _ = cdp
            .call("Browser.close", params, None, Duration::from_secs(1))
            .await;
    }
}
