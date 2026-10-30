use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};
use pi_ext_sdk::{ToolCtx, ToolResult};
use serde_json::{Value, json};

use crate::state::{Chrome, TargetSel};
use crate::tools::{MAX_TEXT, truncate};

const MAX_CDP_BYTES: usize = 256 * 1024;
const KNOWN_KEYS: &[&str] = &[
    "method",
    "params",
    "timeoutMs",
    "targetId",
    "urlIncludes",
    "titleIncludes",
    "background",
];
const FRONT_METHODS: &[&str] = &["Page.bringToFront", "Target.activateTarget"];

pub fn summarize(method: &str, value: &Value) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    if text.len() <= MAX_CDP_BYTES {
        return text;
    }
    let keys = value
        .as_object()
        .map(|o| o.keys().cloned().collect::<Vec<_>>().join(", "))
        .unwrap_or_default();
    format!(
        "{method} returned {} bytes, over the 256 KiB limit. Top-level keys: {keys}",
        text.len()
    )
}

pub async fn cdp(chrome: Arc<Chrome>, args: Value, ctx: ToolCtx) -> Result<ToolResult> {
    let object = args
        .as_object()
        .ok_or_else(|| anyhow!("arguments must be an object"))?;
    let unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|k| !KNOWN_KEYS.contains(k))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "chrome_cdp received unknown top-level parameter(s): {}. Put CDP fields inside \"params\".",
            unknown.join(", ")
        );
    }
    let method = object
        .get("method")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("method is required"))?;
    let host = ctx.host();
    if FRONT_METHODS.contains(&method) && chrome.background(host).await {
        bail!("Background mode blocks {method}. Ask the user to run /chrome background off.");
    }
    let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
    let timeout_ms = object
        .get("timeoutMs")
        .and_then(Value::as_u64)
        .unwrap_or(5_000);
    let timeout = Duration::from_millis(timeout_ms.clamp(1, 120_000));
    let value = if method.starts_with("Browser.") || method.starts_with("Target.") {
        chrome
            .cdp(host)
            .await?
            .call(method, params, None, timeout)
            .await?
    } else {
        let sel: TargetSel = serde_json::from_value(args.clone())?;
        let page = chrome.page(host, &sel).await?;
        let value = page.call_with(method, params, timeout).await;
        page.detach().await;
        value?
    };
    Ok(ToolResult::text(summarize(method, &value)).details(json!({ "method": method })))
}

pub async fn targets(chrome: Arc<Chrome>, sel: TargetSel, ctx: ToolCtx) -> Result<ToolResult> {
    let reply = chrome
        .browser_call(ctx.host(), "Target.getTargets", json!({}))
        .await?;
    let targets: Vec<Value> = reply["targetInfos"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|t| sel.is_empty() || sel.matches(t))
        .collect();
    let mut lines = vec![format!("{} CDP target(s):", targets.len())];
    lines.extend(targets.iter().map(|t| {
        let attached = if t["attached"] == true {
            "attached"
        } else {
            "detached"
        };
        let field = |k: &str| t[k].as_str().unwrap_or("").to_owned();
        format!(
            "- {}\t{attached}\t{}\t{}\t{}",
            field("type"),
            field("targetId"),
            field("title"),
            field("url")
        )
    }));
    let text = truncate(&lines.join("\n"), MAX_TEXT);
    Ok(ToolResult::text(text).details(json!({ "targets": targets })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizes_large_results() {
        let small = json!({ "a": 1 });
        assert!(summarize("X.y", &small).contains("\"a\": 1"));
        let big = json!({ "data": "x".repeat(MAX_CDP_BYTES + 1) });
        let text = summarize("Page.captureScreenshot", &big);
        assert!(text.contains("over the 256 KiB limit") && text.contains("data"));
    }
}
