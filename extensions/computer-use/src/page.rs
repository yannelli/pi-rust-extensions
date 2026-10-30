use base64::Engine;
use std::time::Duration;

use pi_ext_sdk::{CancellationToken, Error, Result};
use serde_json::{Value, json};

use crate::backend::Image;
use crate::cdp::{self, Page};
use crate::outline::{Node, Rect};
use crate::platform::jpeg_size;
use crate::runtime::Runtime;
use crate::state::{Root, Target};

const OUTLINE_JS: &str = include_str!("page.js");

pub fn port(rt: &Runtime) -> Option<u16> {
    rt.browser().as_ref().map(|managed| managed.port)
}

/// Page targets of the managed browser as `browser_page` roots.
pub fn page_roots(list: &Value, browser: &str, pid: i64) -> Vec<Root> {
    let pages = list.as_array().map(Vec::as_slice).unwrap_or_default();
    pages
        .iter()
        .filter(|page| page["type"] == "page")
        .filter_map(|page| {
            Some(Root {
                app: format!("{browser} (managed)"),
                pid,
                window_title: page["title"].as_str().unwrap_or_default().to_owned(),
                kind: Some("browser_page".into()),
                page_id: Some(page["id"].as_str()?.to_owned()),
                url: page["url"].as_str().map(str::to_owned),
                scale_factor: 1.0,
                is_onscreen: true,
                ..Root::default()
            })
        })
        .collect()
}

pub async fn roots(rt: &Runtime) -> Result<Vec<Root>> {
    let Some((port, name, pid)) = rt.browser().as_ref().map(|m| (m.port, m.name, m.pid)) else {
        return Ok(Vec::new());
    };
    Ok(page_roots(
        &cdp::get_json(port, "/json/list").await?,
        name,
        pid,
    ))
}

fn connect_args(target: &Target) -> Result<(u16, &str)> {
    let page = target
        .page_id
        .as_deref()
        .ok_or_else(|| Error::msg("Browser transaction requires a browser observation state."))?;
    let port = target.cdp_port.ok_or_else(|| {
        Error::msg("The managed browser is not running. Call launch_browser first.")
    })?;
    Ok((port, page))
}

pub async fn open(target: &Target, cancel: &CancellationToken) -> Result<Page> {
    let (port, page) = connect_args(target)?;
    Page::connect(port, page, cancel).await
}

/// Runs the outline script and an optional screenshot; frames are viewport CSS pixels.
pub async fn look(
    target: &Target,
    mode: &str,
    cancel: &CancellationToken,
) -> Result<(Target, Node, Option<Image>)> {
    let mut page = open(target, cancel).await?;
    let result = page.evaluate(OUTLINE_JS).await?;
    let mut root: Node = serde_json::from_value(result["root"].clone())
        .map_err(|err| Error::msg(format!("Page outline is invalid: {err}")))?;
    root.assign_refs(&mut 0);
    let viewport = Rect {
        x: 0.0,
        y: 0.0,
        w: result["viewport"]["w"].as_f64().unwrap_or(0.0),
        h: result["viewport"]["h"].as_f64().unwrap_or(0.0),
    };
    let mut next = target.clone();
    next.frame_points = viewport;
    next.window_title = result["title"].as_str().unwrap_or_default().to_owned();
    if mode == "semantic" {
        return Ok((next, root, None));
    }
    let shot = page
        .call(
            "Page.captureScreenshot",
            json!({ "format": "jpeg", "quality": 70 }),
        )
        .await?;
    let data = shot["data"].as_str().unwrap_or_default().to_owned();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(&data)
        .map_err(|err| Error::msg(format!("Screenshot data is invalid: {err}")))?;
    let (width, height) =
        jpeg_size(&bytes).ok_or_else(|| Error::msg("Screenshot is not a JPEG image"))?;
    Ok((
        next,
        root,
        Some(Image {
            base64: data,
            width,
            height,
        }),
    ))
}

/// `Page.navigate`, then waits up to 15s for `document.readyState` to be complete.
pub async fn navigate(target: &Target, url: &str, cancel: &CancellationToken) -> Result<()> {
    navigate_with_timeout(target, url, cancel, Duration::from_secs(15)).await
}

pub(crate) async fn navigate_with_timeout(
    target: &Target,
    url: &str,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<()> {
    let mut page = open(target, cancel).await?;
    let result = page.call("Page.navigate", json!({ "url": url })).await?;
    if let Some(error) = result["errorText"].as_str().filter(|e| !e.is_empty()) {
        return Err(Error::msg(format!("Navigation failed: {error}")));
    }
    let loaded = async {
        loop {
            let ready = page.evaluate("document.readyState").await?;
            if ready.as_str() == Some("complete") {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    };
    tokio::time::timeout(timeout, loaded).await.map_err(|_| {
        Error::msg(format!(
            "Page did not finish loading within {} ms.",
            timeout.as_millis()
        ))
    })?
}
