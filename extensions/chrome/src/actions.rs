use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use pi_ext_sdk::{ToolCtx, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::input::{self, Modifiers, Point};
use crate::page::Page;
use crate::snapshot::SnapshotArgs;
use crate::state::{Chrome, TargetSel};
use crate::tools::take_snapshot;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActionArgs {
    #[serde(flatten)]
    point: Point,
    text: Option<String>,
    replace: Option<bool>,
    per_character: Option<bool>,
    press_enter: Option<bool>,
    submit: Option<bool>,
    dom_fallback: Option<bool>,
    key: Option<String>,
    #[serde(default)]
    modifiers: Modifiers,
    delta_x: Option<f64>,
    delta_y: Option<f64>,
    steps: Option<u32>,
    paths: Option<Vec<String>>,
    include_snapshot: Option<bool>,
    max_elements: Option<u64>,
    #[serde(flatten)]
    sel: TargetSel,
}

#[derive(Clone, Copy)]
pub enum Action {
    Click,
    Type,
    Fill,
    Key,
    Hover,
    Scroll,
    Upload,
}

pub async fn run(
    chrome: Arc<Chrome>,
    action: Action,
    args: ActionArgs,
    ctx: ToolCtx,
) -> Result<ToolResult> {
    let page = chrome.page(ctx.host(), &args.sel).await?;
    let result = act(&chrome, &page, action, &args, &ctx).await;
    page.detach().await;
    result.map(ToolResult::text)
}

async fn act(
    chrome: &Chrome,
    page: &Page,
    action: Action,
    args: &ActionArgs,
    ctx: &ToolCtx,
) -> Result<String> {
    let mut text = match action {
        Action::Click => click(page, args).await?,
        Action::Type => type_text(page, args, args.replace.unwrap_or(false)).await?,
        Action::Fill => type_text(page, args, true).await?,
        Action::Key => {
            let key = args.key.as_deref().unwrap_or_default();
            input::press(page, key, args.modifiers).await?;
            format!("Pressed {key}.")
        }
        Action::Hover => {
            let (x, y) = input::locate(page, &args.point).await?;
            input::mouse(page, "mouseMoved", x, y, json!({})).await?;
            format!("Hovered {}.", args.point.label())
        }
        Action::Scroll => scroll(page, args).await?,
        Action::Upload => upload(page, args, ctx).await?,
    };
    if args.include_snapshot == Some(true) {
        tokio::time::sleep(Duration::from_millis(300)).await;
        let snapshot = SnapshotArgs {
            max_elements: args.max_elements,
            ..SnapshotArgs::default()
        };
        let (snapshot_text, _) = take_snapshot(chrome, page, &snapshot).await?;
        text.push_str("\n\n");
        text.push_str(&snapshot_text);
    }
    Ok(text)
}

async fn click(page: &Page, args: &ActionArgs) -> Result<String> {
    let css = args.point.css()?;
    let trusted = match input::locate(page, &args.point).await {
        Ok((x, y)) => input::click_at(page, x, y).await,
        Err(err) => Err(err),
    };
    match (trusted, css) {
        (Ok(()), _) => Ok(format!("Clicked {}.", args.point.label())),
        (Err(_), Some(css)) if args.dom_fallback.unwrap_or(true) => {
            input::dom_click(page, &css).await?;
            Ok(format!("Clicked {} with a DOM click.", args.point.label()))
        }
        (Err(err), _) => Err(err),
    }
}

async fn type_text(page: &Page, args: &ActionArgs, replace: bool) -> Result<String> {
    let text = args.text.as_deref().unwrap_or_default();
    let css = args.point.css()?;
    if let Some(css) = &css {
        if let Ok((x, y)) = input::locate(page, &args.point).await {
            let _ = input::click_at(page, x, y).await;
        }
        input::focus(page, css, replace).await?;
    } else if replace {
        input::focus(page, ":focus", true).await?;
    }
    let before = input::focused_value(page).await;
    if args.per_character == Some(true) {
        for ch in text.chars() {
            let key = if ch == '\n' {
                "Enter".to_owned()
            } else {
                ch.to_string()
            };
            input::press(page, &key, Modifiers::default()).await?;
        }
    } else if !text.is_empty() {
        input::insert_text(page, text).await?;
    } else if replace {
        input::press(page, "Backspace", Modifiers::default()).await?;
    }
    if args.press_enter == Some(true) || args.submit == Some(true) {
        input::press(page, "Enter", Modifiers::default()).await?;
    }
    let after = input::focused_value(page).await;
    let into = css
        .map(|_| format!(" into {}", args.point.label()))
        .unwrap_or_default();
    let verb = if replace {
        "Replaced the field with"
    } else {
        "Typed"
    };
    let mut out = format!("{verb} {} character(s){into}.", text.chars().count());
    if let (Some(before), Some(after)) = (before, after) {
        out.push_str(&format!(
            "\nField value before: {before:?}\nField value after: {after:?}"
        ));
    }
    Ok(out)
}

async fn scroll(page: &Page, args: &ActionArgs) -> Result<String> {
    let point = &args.point;
    let (x, y) = if point.css()?.is_some() || (point.x.is_some() && point.y.is_some()) {
        input::locate(page, point).await?
    } else {
        let center = page
            .evaluate("[innerWidth / 2, innerHeight / 2]", false)
            .await?;
        (
            center[0].as_f64().unwrap_or(0.0),
            center[1].as_f64().unwrap_or(0.0),
        )
    };
    let (dx, dy) = (args.delta_x.unwrap_or(0.0), args.delta_y.unwrap_or(0.0));
    let default_steps = (dy.abs().max(dx.abs()) / 100.0).ceil() as u32;
    let steps = args.steps.unwrap_or(default_steps).clamp(1, 50);
    for _ in 0..steps {
        let wheel = json!({ "deltaX": dx / f64::from(steps), "deltaY": dy / f64::from(steps) });
        input::mouse(page, "mouseWheel", x, y, wheel).await?;
    }
    let position = page
        .evaluate("[scrollX, scrollY]", false)
        .await
        .unwrap_or_default();
    Ok(format!(
        "Scrolled dy={dy} dx={dx} in {steps} step(s). Page scroll is now x={} y={}.",
        position[0], position[1]
    ))
}

async fn upload(page: &Page, args: &ActionArgs, ctx: &ToolCtx) -> Result<String> {
    let Some(css) = args.point.css()? else {
        bail!("Pass uid or selector of the file input.");
    };
    let cwd = ctx.host().cwd();
    let mut files = Vec::new();
    for path in args.paths.as_deref().unwrap_or_default() {
        let full = cwd.join(path);
        if !full.is_file() {
            bail!("File not found: {}", full.display());
        }
        files.push(full.display().to_string());
    }
    if files.is_empty() {
        bail!("paths is empty.");
    }
    let document = page.call("DOM.getDocument", json!({ "depth": 0 })).await?;
    let root = document["root"]["nodeId"].clone();
    let found = page
        .call(
            "DOM.querySelector",
            json!({ "nodeId": root, "selector": css }),
        )
        .await?;
    let node = found["nodeId"].as_i64().unwrap_or(0);
    if node == 0 {
        bail!("No element matches {}.", args.point.label());
    }
    let params = json!({ "files": files, "nodeId": node });
    page.call("DOM.setFileInputFiles", params).await?;
    Ok(format!(
        "Uploaded {} file(s) to {}.",
        files.len(),
        args.point.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_args_parse_upstream_names() {
        let args: ActionArgs = serde_json::from_value(json!({
            "uid": "e1", "text": "hi", "perCharacter": true, "pressEnter": true,
            "modifiers": { "shiftKey": true }, "deltaY": 300, "includeSnapshot": true,
            "urlIncludes": "example", "background": false
        }))
        .unwrap();
        assert_eq!(args.point.uid.as_deref(), Some("e1"));
        assert!(args.modifiers.shift_key && args.per_character == Some(true));
        assert_eq!(args.sel.url_includes.as_deref(), Some("example"));
        assert_eq!(args.sel.background, Some(false));
    }
}
