use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use pi_ext_sdk::{ToolCtx, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::auth::epoch_ms;
use crate::state::{Chrome, TargetSel};

const KEEP_NEWEST: usize = 20;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScreenshotArgs {
    path: Option<String>,
    format: Option<String>,
    quality: Option<u8>,
    full_page: Option<bool>,
    retention_days: Option<u64>,
    #[serde(flatten)]
    sel: TargetSel,
}

pub async fn run(chrome: Arc<Chrome>, args: ScreenshotArgs, ctx: ToolCtx) -> Result<ToolResult> {
    let format = args.format.as_deref().unwrap_or("png");
    if !matches!(format, "png" | "jpeg") {
        bail!("format must be png or jpeg");
    }
    let cwd = ctx.host().cwd();
    let dir = cwd.join(".pi").join("chrome-screenshots");
    let output = match &args.path {
        Some(path) => cwd.join(path),
        None => dir.join(format!("{}.{format}", epoch_ms(SystemTime::now()))),
    };
    let mut params = json!({ "format": format, "fromSurface": true });
    if format == "jpeg" {
        params["quality"] = json!(args.quality.unwrap_or(80).min(100));
    }
    let page = chrome.page(ctx.host(), &args.sel).await?;
    let capture = async {
        if args.full_page == Some(true) {
            let metrics = page.call("Page.getLayoutMetrics", json!({})).await?;
            let size = metrics
                .get("cssContentSize")
                .or_else(|| metrics.get("contentSize"))
                .cloned()
                .unwrap_or_default();
            params["captureBeyondViewport"] = json!(true);
            params["clip"] = json!({
                "x": 0, "y": 0, "scale": 1,
                "width": size["width"].as_f64().unwrap_or(1280.0),
                "height": size["height"].as_f64().unwrap_or(800.0),
            });
        }
        page.call_with("Page.captureScreenshot", params, Duration::from_secs(60))
            .await
    };
    let reply = capture.await;
    page.detach().await;
    let data = reply?["data"]
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("Page.captureScreenshot returned no data"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data)
        .context("screenshot data is not base64")?;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&output, &bytes).with_context(|| format!("{}", output.display()))?;
    if args.path.is_none() {
        let days = args.retention_days.unwrap_or(7);
        prune(&dir, days, SystemTime::now());
    }
    let text = format!("Saved Chrome screenshot to {}", output.display());
    let details = json!({ "path": output, "format": format, "bytes": bytes.len() });
    Ok(ToolResult::text(text).details(details))
}

fn is_capture(path: &Path) -> bool {
    let (Some(stem), Some(ext)) = (path.file_stem(), path.extension()) else {
        return false;
    };
    let digits = stem
        .to_str()
        .is_some_and(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()));
    digits && matches!(ext.to_str(), Some("png" | "jpeg"))
}

/// Deletes captures older than `days`, except the newest `KEEP_NEWEST`. `0` keeps all.
pub fn prune(dir: &Path, days: u64, now: SystemTime) -> usize {
    if days == 0 {
        return 0;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()) && is_capture(&e.path()))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by_key(|file| std::cmp::Reverse(file.0));
    let max_age = Duration::from_secs(days.saturating_mul(86_400));
    let mut removed = 0;
    for (modified, path) in files.into_iter().skip(KEEP_NEWEST) {
        let old = now.duration_since(modified).is_ok_and(|age| age > max_age);
        if old && std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prune_keeps_newest_and_recent() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..25 {
            std::fs::write(dir.path().join(format!("{}.png", 1_000 + i)), b"x").unwrap();
        }
        std::fs::write(dir.path().join("notes.png"), b"x").unwrap();
        std::fs::write(dir.path().join("1700.txt"), b"x").unwrap();
        let later = SystemTime::now() + Duration::from_secs(30 * 86_400);
        assert_eq!(prune(dir.path(), 0, later), 0);
        assert_eq!(prune(dir.path(), 7, SystemTime::now()), 0);
        assert_eq!(prune(dir.path(), 7, later), 5);
        assert_eq!(
            std::fs::read_dir(dir.path()).unwrap().count(),
            KEEP_NEWEST + 2
        );
        assert!(dir.path().join("notes.png").exists() && dir.path().join("1700.txt").exists());
    }
}
