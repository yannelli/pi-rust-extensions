use pi_ext_sdk::{CancellationToken, Error, Result};
use serde_json::{Value, json};

use crate::act::{UiAction, stale_ref};
use crate::cdp::Page;
use crate::page::open;
use crate::platform::{expand_keys, image_to_screen};
use crate::state::Observation;

pub struct KeyEvent {
    pub key: String,
    pub code: String,
    pub vk: u32,
    pub text: Option<String>,
}

pub fn key_event(name: &str) -> Option<KeyEvent> {
    let named = |key: &str, code: &str, vk: u32| KeyEvent {
        key: key.into(),
        code: code.into(),
        vk,
        text: None,
    };
    Some(match name {
        "enter" | "return" => KeyEvent {
            text: Some("\r".into()),
            ..named("Enter", "Enter", 13)
        },
        "tab" => named("Tab", "Tab", 9),
        "escape" | "esc" => named("Escape", "Escape", 27),
        "backspace" => named("Backspace", "Backspace", 8),
        "delete" | "forwarddelete" => named("Delete", "Delete", 46),
        "space" => KeyEvent {
            text: Some(" ".into()),
            ..named(" ", "Space", 32)
        },
        "up" | "arrowup" => named("ArrowUp", "ArrowUp", 38),
        "down" | "arrowdown" => named("ArrowDown", "ArrowDown", 40),
        "left" | "arrowleft" => named("ArrowLeft", "ArrowLeft", 37),
        "right" | "arrowright" => named("ArrowRight", "ArrowRight", 39),
        "home" => named("Home", "Home", 36),
        "end" => named("End", "End", 35),
        "pageup" => named("PageUp", "PageUp", 33),
        "pagedown" => named("PageDown", "PageDown", 34),
        other if other.len() == 1 => {
            let ch = other.chars().next()?;
            let upper = ch.to_ascii_uppercase();
            let code = if ch.is_ascii_digit() {
                format!("Digit{ch}")
            } else if ch.is_ascii_alphabetic() {
                format!("Key{upper}")
            } else {
                String::new()
            };
            KeyEvent {
                key: other.into(),
                code,
                vk: u32::from(upper),
                text: Some(other.into()),
            }
        }
        other
            if other.starts_with('f')
                && other[1..]
                    .parse::<u32>()
                    .is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let n: u32 = other[1..].parse().ok()?;
            named(&other.to_uppercase(), &other.to_uppercase(), 111 + n)
        }
        _ => return None,
    })
}

/// CDP modifier bits: Alt 1, Ctrl 2, Meta 4, Shift 8.
pub fn modifier_bit(name: &str) -> Option<u32> {
    match name {
        "alt" | "option" | "opt" => Some(1),
        "ctrl" | "control" => Some(2),
        "cmd" | "command" | "meta" | "super" => Some(4),
        "shift" => Some(8),
        _ => None,
    }
}

async fn keys(page: &mut Page, raw: &[String]) -> Result<()> {
    let keys = expand_keys(raw);
    let bits: Vec<u32> = keys
        .iter()
        .take(keys.len().saturating_sub(1))
        .filter_map(|k| modifier_bit(k))
        .collect();
    let chord = keys.len() >= 2 && bits.len() == keys.len() - 1;
    let (modifiers, list) = if chord {
        (bits.iter().sum(), &keys[keys.len() - 1..])
    } else {
        (0, &keys[..])
    };
    for name in list {
        let event = key_event(name).ok_or_else(|| Error::msg(format!("Unknown key '{name}'")))?;
        let text = if modifiers & 7 == 0 {
            event.text.clone()
        } else {
            None
        };
        let down = json!({ "type": if text.is_some() { "keyDown" } else { "rawKeyDown" }, "key": event.key, "code": event.code,
            "windowsVirtualKeyCode": event.vk, "modifiers": modifiers, "text": text });
        page.call("Input.dispatchKeyEvent", down).await?;
        let up = json!({ "type": "keyUp", "key": event.key, "code": event.code, "windowsVirtualKeyCode": event.vk, "modifiers": modifiers });
        page.call("Input.dispatchKeyEvent", up).await?;
    }
    Ok(())
}

async fn mouse(
    page: &mut Page,
    kind: &str,
    (x, y): (f64, f64),
    button: &str,
    count: u32,
) -> Result<()> {
    let params = json!({ "type": kind, "x": x, "y": y, "button": button, "clickCount": count, "buttons": if kind == "mouseMoved" && button == "none" { 0 } else { 1 } });
    page.call("Input.dispatchMouseEvent", params)
        .await
        .map(|_| ())
}

fn element_index(base: &Observation, reference: &str) -> Result<usize> {
    let node = base
        .outline
        .root
        .find(reference.trim())
        .ok_or_else(|| Error::msg(stale_ref(reference)))?;
    node.path.first().copied().ok_or_else(|| {
        Error::msg(format!(
            "Outline ref '{reference}' is the page root; choose an element ref."
        ))
    })
}

async fn center(page: &mut Page, index: usize) -> Result<(f64, f64)> {
    let expression = format!(
        "(() => {{ const el = window.__piCuNodes && window.__piCuNodes[{index}]; if (!el || !el.isConnected) throw new Error('Browser element is stale. Observe the page again.'); el.scrollIntoView({{ block: 'center', inline: 'center' }}); const r = el.getBoundingClientRect(); return [r.left + r.width / 2, r.top + r.height / 2]; }})()"
    );
    let value = page.evaluate(&expression).await?;
    Ok((
        value[0].as_f64().unwrap_or(0.0),
        value[1].as_f64().unwrap_or(0.0),
    ))
}

fn point(base: &Observation, x: f64, y: f64) -> Result<(f64, f64)> {
    let capture = &base.capture;
    if capture.width == 0
        || x < 0.0
        || y < 0.0
        || x > f64::from(capture.width)
        || y > f64::from(capture.height)
    {
        return Err(Error::msg(format!(
            "Coordinates ({x},{y}) are outside the latest page image ({}x{}). Observe with mode fused or visual and retry.",
            capture.width, capture.height
        )));
    }
    Ok(image_to_screen(
        &base.target.frame_points,
        capture.width,
        capture.height,
        x,
        y,
    ))
}

async fn one(page: &mut Page, base: &Observation, action: &UiAction) -> Result<()> {
    let reference = action.reference.as_deref().filter(|r| !r.trim().is_empty());
    let index = reference.map(|r| element_index(base, r)).transpose()?;
    let button = action.button.as_deref().unwrap_or("left");
    let count = action.click_count.unwrap_or(1.0).round().clamp(1.0, 3.0) as u32;
    match (action.action.as_str(), index) {
        ("press", Some(i)) => page
            .evaluate(&format!("window.__piCu.press({i})"))
            .await
            .map(|_| ()),
        ("click", Some(i)) if button == "left" && count == 1 => page
            .evaluate(&format!("window.__piCu.press({i})"))
            .await
            .map(|_| ()),
        ("click", _) | ("press", None) => {
            let at = match (index, action.x, action.y) {
                (Some(i), _, _) => center(page, i).await?,
                (None, Some(x), Some(y)) => point(base, x, y)?,
                _ => {
                    return Err(Error::msg(format!(
                        "{} requires either ref or both x and y.",
                        action.action
                    )));
                }
            };
            mouse(page, "mouseMoved", at, "none", 0).await?;
            mouse(page, "mousePressed", at, button, count).await?;
            mouse(page, "mouseReleased", at, button, count).await
        }
        ("setText", Some(i)) => {
            let text =
                serde_json::to_string(&action.text.clone().unwrap_or_default()).unwrap_or_default();
            page.evaluate(&format!("window.__piCu.setText({i}, {text})"))
                .await
                .map(|_| ())
        }
        ("setText", None) => Err(Error::msg("setText requires an editable ref.")),
        ("typeText" | "keypress", focus) => {
            if let Some(i) = focus {
                page.evaluate(&format!("window.__piCu.focus({i})")).await?;
            }
            if action.action == "keypress" {
                return keys(page, action.keys.as_deref().unwrap_or_default()).await;
            }
            page.call(
                "Input.insertText",
                json!({ "text": action.text.clone().unwrap_or_default() }),
            )
            .await
            .map(|_| ())
        }
        ("scroll", _) => {
            let at = match index {
                Some(i) => center(page, i).await?,
                None => base.target.frame_points.center(),
            };
            let delta = |v: Option<f64>| v.unwrap_or(0.0).clamp(-10_000.0, 10_000.0);
            let params = json!({ "type": "mouseWheel", "x": at.0, "y": at.1, "deltaX": delta(action.scroll_x), "deltaY": delta(action.scroll_y) });
            page.call("Input.dispatchMouseEvent", params)
                .await
                .map(|_| ())
        }
        ("drag", _) => {
            let path = action.path.as_deref().unwrap_or_default();
            if path.len() < 2 {
                return Err(Error::msg("drag.path must contain at least two points."));
            }
            let points: Vec<(f64, f64)> = path
                .iter()
                .map(|p| point(base, p.x, p.y))
                .collect::<Result<_>>()?;
            mouse(page, "mousePressed", points[0], "left", 1).await?;
            let mut moved = Ok(());
            for at in &points[1..] {
                moved = mouse(page, "mouseMoved", *at, "left", 0).await;
                if moved.is_err() {
                    break;
                }
            }
            let released = mouse(page, "mouseReleased", points[points.len() - 1], "left", 1).await;
            moved.and(released)
        }
        ("moveMouse", _) => {
            let (Some(x), Some(y)) = (action.x, action.y) else {
                return Err(Error::msg("moveMouse requires x and y."));
            };
            mouse(page, "mouseMoved", point(base, x, y)?, "none", 0).await
        }
        (other, _) => Err(Error::msg(format!(
            "Browser transactions do not support '{other}'."
        ))),
    }
}

/// Runs actions in order through CDP input; stops at the first failure.
pub async fn act(
    base: &Observation,
    actions: &[UiAction],
    cancel: &CancellationToken,
) -> Result<Vec<Value>> {
    if actions.is_empty() || actions.len() > 20 {
        return Err(Error::msg("act_ui.actions must contain 1 to 20 actions."));
    }
    for reference in actions
        .iter()
        .filter_map(|a| a.reference.as_deref())
        .filter(|r| !r.trim().is_empty())
    {
        element_index(base, reference)?;
    }
    let mut page = open(&base.target, cancel).await?;
    let mut outcomes = Vec::new();
    for (index, action) in actions.iter().enumerate() {
        match one(&mut page, base, action).await {
            Ok(()) => {
                outcomes.push(json!({ "index": index, "op": action.action, "outcome": "worked" }))
            }
            Err(err) => {
                outcomes.push(json!({ "index": index, "op": action.action, "outcome": "didnt", "error": err.message() }));
                break;
            }
        }
    }
    Ok(outcomes)
}
