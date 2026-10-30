use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use pi_ext_sdk::{ToolCtx, ToolResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::browser;
use crate::state::{Chrome, TargetSel};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LaunchArgs {
    port: Option<u16>,
    url: Option<String>,
    user_data_dir: Option<String>,
    use_default_profile: Option<bool>,
    headless: Option<bool>,
}

pub async fn launch(chrome: Arc<Chrome>, args: LaunchArgs, ctx: ToolCtx) -> Result<ToolResult> {
    if args.use_default_profile == Some(true) {
        bail!(
            "Chrome blocks remote debugging of the default profile. Use the managed profile, or start Chrome with --remote-debugging-port and a --user-data-dir."
        );
    }
    let host = ctx.host();
    let mut config = chrome.config(host).await?;
    let before = config.clone();
    config.port = args.port.unwrap_or(config.port);
    config.headless = args.headless.unwrap_or(config.headless);
    if let Some(dir) = args.user_data_dir {
        config.user_data_dir = host.cwd().join(dir);
    }
    if config != before {
        chrome.set_config(config.clone());
        chrome.disconnect().await;
    }
    let cdp = chrome.ensure(host, true).await?;
    let version = cdp
        .call(
            "Browser.getVersion",
            json!({}),
            None,
            Duration::from_secs(5),
        )
        .await?;
    let ws = chrome.browser_ws().unwrap_or_default();
    let managed = browser::launched_here(&Chrome::state_dir(host), &ws, false);
    let mut text = format!(
        "Connected to {} on {}:{} ({}).",
        version["product"].as_str().unwrap_or("Chrome"),
        config.host,
        config.port,
        if managed {
            "managed profile"
        } else {
            "existing Chrome"
        }
    );
    if let Some(url) = args.url {
        let id = chrome.new_tab(host, &url).await?;
        text.push_str(&format!("\nOpened {url} in tab {id}."));
    }
    Ok(ToolResult::text(text).details(json!({ "version": version, "managed": managed })))
}

#[derive(Deserialize)]
pub struct TabArgs {
    action: String,
    url: Option<String>,
    #[serde(flatten)]
    sel: TargetSel,
}

async fn existing_tab(chrome: &Chrome, ctx: &ToolCtx, sel: &TargetSel) -> Result<String> {
    if !sel.is_empty() {
        return chrome.resolve(ctx.host(), sel).await;
    }
    let pages = chrome.pages(ctx.host()).await?;
    let owned = chrome.owned();
    let live = owned
        .iter()
        .rev()
        .find(|id| pages.iter().any(|p| p["targetId"] == id.as_str()));
    live.cloned().ok_or_else(|| {
        anyhow!("This session has no tab. Pass targetId, urlIncludes, or titleIncludes.")
    })
}

fn tab_line(page: &Value, owned: &[String]) -> String {
    let id = page["targetId"].as_str().unwrap_or("");
    let mark = if owned.iter().any(|o| o == id) {
        "[session] "
    } else {
        ""
    };
    let title = page["title"]
        .as_str()
        .filter(|t| !t.is_empty())
        .unwrap_or("(untitled)");
    format!(
        "{id}\t{mark}{title}\t{}",
        page["url"].as_str().unwrap_or("")
    )
}

pub async fn tab(chrome: Arc<Chrome>, args: TabArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let host = ctx.host();
    let background = chrome.background(host).await;
    match args.action.as_str() {
        "list" => {
            let pages = chrome.pages(host).await?;
            let owned = chrome.owned();
            let lines: Vec<String> = pages.iter().map(|p| tab_line(p, &owned)).collect();
            let text = if lines.is_empty() {
                "No tabs.".to_owned()
            } else {
                lines.join("\n")
            };
            Ok(ToolResult::text(text).details(json!({ "tabs": pages })))
        }
        "new" => {
            let url = args.url.as_deref().unwrap_or("about:blank");
            let id = chrome.new_tab(host, url).await?;
            if !background {
                let params = json!({ "targetId": id });
                chrome
                    .browser_call(host, "Target.activateTarget", params)
                    .await?;
            }
            let text = format!("Opened {url} in tab {id}.");
            Ok(ToolResult::text(text).details(json!({ "targetId": id })))
        }
        "activate" if background => {
            bail!("Background mode is on. Ask the user to run /chrome background off.")
        }
        "activate" => {
            let id = existing_tab(&chrome, &ctx, &args.sel).await?;
            let params = json!({ "targetId": id });
            chrome
                .browser_call(host, "Target.activateTarget", params)
                .await?;
            Ok(ToolResult::text(format!("Activated tab {id}.")))
        }
        "close" => {
            let id = existing_tab(&chrome, &ctx, &args.sel).await?;
            let params = json!({ "targetId": id });
            chrome
                .browser_call(host, "Target.closeTarget", params)
                .await?;
            chrome.disown(&id);
            Ok(ToolResult::text(format!("Closed tab {id}.")))
        }
        "version" => {
            let version = chrome
                .browser_call(host, "Browser.getVersion", json!({}))
                .await?;
            Ok(ToolResult::text(serde_json::to_string_pretty(&version)?).details(version))
        }
        other => bail!("Unknown action {other:?}. Use list, new, activate, close, or version."),
    }
}
