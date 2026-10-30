use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use pi_ext_sdk::{ToolCtx, ToolResult};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::page::Page;
use crate::snapshot::{self, SnapshotArgs};
use crate::state::{Chrome, TargetSel};

pub const MAX_TEXT: usize = 20_000;

pub fn truncate(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((cut, _)) => format!("{}\n[truncated at {max} characters]", &text[..cut]),
        None => text.to_owned(),
    }
}

/// Stores the element lines per tab so `changes` mode can diff against them.
pub async fn take_snapshot(
    chrome: &Chrome,
    page: &Page,
    args: &SnapshotArgs,
) -> Result<(String, Value)> {
    let value = page.evaluate(&args.expression(), false).await?;
    let lines = snapshot::element_lines(&value);
    let mut stored = chrome.snapshots.lock().expect("snapshots");
    let previous = stored.get(&page.target_id).cloned();
    let text = snapshot::format(&value, args.mode(), previous.as_deref());
    if args.mode() != "text" {
        stored.insert(page.target_id.clone(), lines);
    }
    Ok((text, value))
}

#[derive(Deserialize)]
pub struct SnapshotToolArgs {
    #[serde(flatten)]
    snapshot: SnapshotArgs,
    #[serde(flatten)]
    sel: TargetSel,
}

pub async fn snapshot(
    chrome: Arc<Chrome>,
    args: SnapshotToolArgs,
    ctx: ToolCtx,
) -> Result<ToolResult> {
    let page = chrome.page(ctx.host(), &args.sel).await?;
    let result = take_snapshot(&chrome, &page, &args.snapshot).await;
    page.detach().await;
    let (text, value) = result?;
    Ok(ToolResult::text(text).details(json!({ "snapshot": value })))
}

pub async fn find(chrome: Arc<Chrome>, args: SnapshotToolArgs, ctx: ToolCtx) -> Result<ToolResult> {
    if args.snapshot.query.as_deref().is_none_or(str::is_empty) {
        bail!("query is required");
    }
    snapshot(chrome, args, ctx).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NavigateArgs {
    url: String,
    wait_until_load: Option<bool>,
    timeout_ms: Option<u64>,
    init_script: Option<String>,
    #[serde(flatten)]
    sel: TargetSel,
}

pub async fn navigate(chrome: Arc<Chrome>, args: NavigateArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let page = chrome.page(ctx.host(), &args.sel).await?;
    let result = navigate_on(&page, &args).await;
    page.detach().await;
    result
}

async fn navigate_on(page: &Page, args: &NavigateArgs) -> Result<ToolResult> {
    if let Some(source) = &args.init_script {
        page.call("Page.enable", json!({})).await?;
        let params = json!({ "source": source });
        page.call("Page.addScriptToEvaluateOnNewDocument", params)
            .await?;
    }
    let reply = page
        .call("Page.navigate", json!({ "url": args.url }))
        .await?;
    if let Some(error) = reply["errorText"].as_str().filter(|e| !e.is_empty()) {
        bail!("Navigation to {} failed: {error}", args.url);
    }
    let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(15_000).min(120_000));
    if args.wait_until_load.unwrap_or(true) {
        let ready = "document.readyState === 'complete'";
        poll(page, ready, timeout, Duration::from_millis(100)).await?;
    }
    let state = page
        .evaluate("[location.href, document.title]", false)
        .await;
    let state = state.unwrap_or_default();
    let text = format!(
        "Navigated to {}{}\nTitle: {}",
        state[0].as_str().unwrap_or(&args.url),
        if args.init_script.is_some() {
            " (with initScript)"
        } else {
            ""
        },
        state[1].as_str().unwrap_or("")
    );
    let details = json!({ "frameId": reply["frameId"], "loaderId": reply["loaderId"] });
    Ok(ToolResult::text(text).details(details))
}

async fn poll(page: &Page, expression: &str, timeout: Duration, interval: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        let value = page.evaluate(expression, true).await.ok();
        if value.and_then(|v| v.as_bool()) == Some(true) {
            return Ok(());
        }
        if start.elapsed() >= timeout {
            bail!(
                "Timed out after {} ms waiting for {expression}",
                timeout.as_millis()
            );
        }
        tokio::time::sleep(interval).await;
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvaluateArgs {
    expression: String,
    await_promise: Option<bool>,
    #[serde(flatten)]
    sel: TargetSel,
}

pub async fn evaluate(chrome: Arc<Chrome>, args: EvaluateArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let page = chrome.page(ctx.host(), &args.sel).await?;
    let await_promise = args.await_promise.unwrap_or(true);
    let value = page.evaluate(&args.expression, await_promise).await;
    page.detach().await;
    let value = value?;
    let text = match &value {
        Value::Null => "undefined".to_owned(),
        Value::String(text) => text.clone(),
        other => serde_json::to_string_pretty(other)?,
    };
    Ok(ToolResult::text(truncate(&text, MAX_TEXT)).details(json!({ "value": value })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitArgs {
    kind: String,
    value: String,
    timeout_ms: Option<u64>,
    interval_ms: Option<u64>,
    #[serde(flatten)]
    sel: TargetSel,
}

pub async fn wait_for(chrome: Arc<Chrome>, args: WaitArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let expression = match args.kind.as_str() {
        "selector" => format!("!!document.querySelector({})", json!(args.value)),
        "expression" => format!("!!({})", args.value),
        other => bail!("Unknown kind {other:?}. Use selector or expression."),
    };
    let timeout = Duration::from_millis(args.timeout_ms.unwrap_or(10_000).min(120_000));
    let interval = Duration::from_millis(args.interval_ms.unwrap_or(250).clamp(20, 5_000));
    let page = chrome.page(ctx.host(), &args.sel).await?;
    let result = poll(&page, &expression, timeout, interval).await;
    page.detach().await;
    result?;
    Ok(ToolResult::text(format!(
        "Observed {}: {}",
        args.kind, args.value
    )))
}
