use anyhow::{Result, anyhow, bail};
use serde::Deserialize;
use serde_json::{Value, json};

pub use crate::keys::{Modifiers, press};
use crate::page::Page;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Point {
    pub uid: Option<String>,
    pub selector: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

impl Point {
    pub fn css(&self) -> Result<Option<String>> {
        if let Some(uid) = &self.uid {
            if uid.is_empty() || !uid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                bail!("invalid uid {uid:?}");
            }
            return Ok(Some(format!("[data-pi-uid=\"{uid}\"]")));
        }
        Ok(self.selector.clone())
    }

    pub fn label(&self) -> String {
        self.uid
            .clone()
            .or_else(|| self.selector.clone())
            .unwrap_or_else(|| {
                format!(
                    "{},{}",
                    self.x.unwrap_or_default(),
                    self.y.unwrap_or_default()
                )
            })
    }
}

/// Scrolls the element into view and returns its center, or the given coordinates.
pub async fn locate(page: &Page, point: &Point) -> Result<(f64, f64)> {
    let Some(css) = point.css()? else {
        return match (point.x, point.y) {
            (Some(x), Some(y)) => Ok((x, y)),
            _ => bail!("Pass uid, selector, or both x and y."),
        };
    };
    let script = format!(
        "(() => {{ const el = document.querySelector({css}); if (!el) return null; \
         el.scrollIntoView({{ block: 'center', inline: 'center' }}); \
         const r = el.getBoundingClientRect(); return [r.x + r.width / 2, r.y + r.height / 2]; }})()",
        css = json!(css)
    );
    let value = page.evaluate(&script, false).await?;
    match (value[0].as_f64(), value[1].as_f64()) {
        (Some(x), Some(y)) => Ok((x, y)),
        _ => Err(anyhow!(
            "No element matches {}. Take a new chrome_snapshot.",
            point.label()
        )),
    }
}

pub async fn mouse(page: &Page, kind: &str, x: f64, y: f64, extra: Value) -> Result<()> {
    let mut params = json!({ "type": kind, "x": x, "y": y });
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        params.extend(extra.clone());
    }
    page.call("Input.dispatchMouseEvent", params)
        .await
        .map(drop)
}

pub async fn click_at(page: &Page, x: f64, y: f64) -> Result<()> {
    let press = json!({ "button": "left", "buttons": 1, "clickCount": 1 });
    mouse(page, "mouseMoved", x, y, json!({})).await?;
    mouse(page, "mousePressed", x, y, press.clone()).await?;
    mouse(page, "mouseReleased", x, y, press).await
}

/// Runs a DOM `click()` on the element; used when trusted input fails.
pub async fn dom_click(page: &Page, css: &str) -> Result<()> {
    let script = format!(
        "(() => {{ const el = document.querySelector({}); if (!el) return false; el.click(); return true; }})()",
        json!(css)
    );
    match page.evaluate(&script, false).await?.as_bool() {
        Some(true) => Ok(()),
        _ => bail!("No element matches {css}"),
    }
}

/// Focuses the element and puts the caret at the end, or selects all when `select_all`.
pub async fn focus(page: &Page, css: &str, select_all: bool) -> Result<()> {
    let script = format!(
        "(() => {{ const el = document.querySelector({css}); if (!el) return false; el.focus(); \
         const all = {select_all}; \
         if (typeof el.value === 'string' && el.setSelectionRange) {{ \
           try {{ el.setSelectionRange(all ? 0 : el.value.length, el.value.length); }} catch (_) {{ if (all) el.select(); }} \
         }} else if (el.isContentEditable) {{ const r = document.createRange(); r.selectNodeContents(el); \
           if (!all) r.collapse(false); const s = getSelection(); s.removeAllRanges(); s.addRange(r); }} \
         return true; }})()",
        css = json!(css)
    );
    match page.evaluate(&script, false).await?.as_bool() {
        Some(true) => Ok(()),
        _ => bail!("No element matches {css}"),
    }
}

pub async fn focused_value(page: &Page) -> Option<String> {
    let script = "(() => { const el = document.activeElement; if (!el) return null; \
                  return typeof el.value === 'string' ? el.value : el.isContentEditable ? el.innerText : null; })()";
    let value = page.evaluate(script, false).await.ok()?;
    value.as_str().map(|s| s.chars().take(200).collect())
}

pub async fn insert_text(page: &Page, text: &str) -> Result<()> {
    page.call("Input.insertText", json!({ "text": text }))
        .await
        .map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_css_validates_uid() {
        let point = Point {
            uid: Some("e12".into()),
            ..Point::default()
        };
        assert_eq!(
            point.css().unwrap().as_deref(),
            Some("[data-pi-uid=\"e12\"]")
        );
        let bad = Point {
            uid: Some("e1\"]".into()),
            ..Point::default()
        };
        assert!(bad.css().is_err());
        let xy = Point {
            x: Some(1.0),
            y: Some(2.0),
            ..Point::default()
        };
        assert_eq!((xy.css().unwrap(), xy.label()), (None, "1,2".to_owned()));
    }
}
