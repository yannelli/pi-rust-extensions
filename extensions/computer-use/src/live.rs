use pi_ext_sdk::{CancellationToken, Error, Host, Result, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::backend::{Backend, Image, Kind};
use crate::config::is_browser;
use crate::outline::Outline;
use crate::page;
use crate::runtime::Runtime;
use crate::setup::ensure_ready;
use crate::state::{Capture, Observation, Root, Target, now_ms};

pub struct Live<'a> {
    pub host: &'a Host,
    pub rt: &'a Runtime,
    pub cancel: &'a CancellationToken,
}

pub fn browser_refused(app: &str) -> String {
    format!(
        "browser_use is disabled in the computer-use config, so '{app}' is refused. Set browser_use to true to control browsers."
    )
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FindArgs {
    pub text: Option<String>,
    pub app: Option<String>,
    pub bundle_id: Option<String>,
    pub pid: Option<f64>,
    pub kind: Option<String>,
}

fn rank(root: &Root, text: Option<&str>) -> Option<f64> {
    let base = if root.is_focused { 0.5 } else { 0.0 };
    let Some(text) = text.map(str::to_lowercase).filter(|t| !t.is_empty()) else {
        return Some(base);
    };
    let fields = [
        root.app.to_lowercase(),
        root.window_title.to_lowercase(),
        root.url.clone().unwrap_or_default().to_lowercase(),
    ];
    let score = fields.iter().map(|f| {
        if *f == text {
            3.0
        } else if f.starts_with(&text) {
            2.0
        } else if f.contains(&text) {
            1.0
        } else {
            0.0
        }
    });
    let best = score.fold(0.0, f64::max);
    (best > 0.0).then_some(best + base)
}

/// Filters and ranks roots; at most 20 are returned.
pub fn select_roots(roots: Vec<Root>, args: &FindArgs, browser_use: bool) -> Vec<Root> {
    let kind = args.kind.as_deref().unwrap_or_default();
    let mut ranked: Vec<(f64, Root)> = roots
        .into_iter()
        .filter(|r| {
            browser_use || r.page_id.is_some() || !is_browser(&r.app, r.bundle_id.as_deref())
        })
        .filter(|r| kind.is_empty() || r.kind.as_deref().unwrap_or("window") == kind)
        .filter(|r| {
            args.app
                .as_deref()
                .is_none_or(|a| r.app.eq_ignore_ascii_case(a.trim()))
        })
        .filter(|r| {
            args.bundle_id
                .as_deref()
                .is_none_or(|b| r.bundle_id.as_deref() == Some(b.trim()))
        })
        .filter(|r| args.pid.is_none_or(|p| r.pid == p as i64))
        .filter_map(|r| rank(&r, args.text.as_deref()).map(|score| (score, r)))
        .collect();
    ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
    ranked.into_iter().take(20).map(|(_, root)| root).collect()
}

fn root_line(root: &Root) -> String {
    let f = root.frame_points;
    let focus = if root.is_focused { " (focused)" } else { "" };
    let kind = root.kind.as_deref().unwrap_or("window");
    let title = serde_json::to_string(&root.window_title).unwrap_or_default();
    format!(
        "{} {kind} {} — {title} pid={} frame={},{} {}x{}{focus}",
        root.window_ref, root.app, root.pid, f.x, f.y, f.w, f.h
    )
}

impl Live<'_> {
    async fn desktop(&self) -> Result<Backend> {
        ensure_ready(self.host, self.cancel).await
    }

    pub async fn find_roots(&self, args: FindArgs) -> Result<ToolResult> {
        let config = self.rt.config();
        let mut roots = Vec::new();
        if args.kind.as_deref() != Some("browser_page") {
            roots.extend(
                self.desktop()
                    .await?
                    .windows()
                    .await?
                    .into_iter()
                    .map(|r| Root {
                        kind: Some("window".into()),
                        ..r
                    }),
            );
        }
        if config.browser_use {
            roots.extend(page::roots(self.rt).await.unwrap_or_default());
        }
        let selected = select_roots(roots, &args, config.browser_use);
        let selected: Vec<Root> = {
            let mut store = self.rt.store();
            selected
                .into_iter()
                .map(|root| store.put_root(root))
                .collect()
        };
        let mut lines = vec![format!(
            "Found {} root{}.",
            selected.len(),
            if selected.len() == 1 { "" } else { "s" }
        )];
        lines.extend(selected.iter().map(root_line));
        Ok(ToolResult::text(lines.join("\n"))
            .details(json!({ "tool": "find_roots", "windows": selected })))
    }

    /// The `@r` root, or the focused desktop window when `root` is absent.
    pub async fn resolve(&self, root: Option<&str>) -> Result<Target> {
        let root = root.map(str::trim).filter(|r| !r.is_empty());
        if let Some(reference) = root {
            let valid = reference
                .strip_prefix("@r")
                .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            if !valid {
                return Err(Error::msg(
                    "observe_ui.root must be an exact @r ref issued by find_roots.",
                ));
            }
            let known = self.rt.store().roots.get(reference).cloned();
            let known = known.ok_or_else(|| Error::msg(format!("Root ref '{reference}' is not available in this session. Call find_roots first.")))?;
            if known.page_id.is_some() {
                return Ok(Target::from_root(&known, page::port(self.rt)));
            }
            return self.refresh(&Target::from_root(&known, None)).await?.ok_or_else(|| {
                Error::msg(format!("Root ref '{reference}' is stale. Call find_roots again and choose a current window."))
            });
        }
        let windows = self.desktop().await?.windows().await?;
        let browser_use = self.rt.config().browser_use;
        let usable = windows
            .into_iter()
            .filter(|w| browser_use || !is_browser(&w.app, w.bundle_id.as_deref()));
        let mut usable: Vec<Root> = usable.collect();
        usable.sort_by_key(|w| !w.is_focused);
        let front = usable.into_iter().next();
        let front = front.ok_or_else(|| Error::msg("No frontmost controllable root was found. Open an app window and call observe_ui again."))?;
        let root = self.rt.store().put_root(front);
        Ok(Target::from_root(&root, None))
    }

    /// Re-reads the window frame; `None` when the window is gone.
    pub async fn refresh(&self, target: &Target) -> Result<Option<Target>> {
        if target.page_id.is_some() {
            return Ok(Some(target.clone()));
        }
        let windows = self.desktop().await?.windows().await?;
        let found = windows
            .into_iter()
            .find(|w| w.pid == target.pid && w.window_id == target.window_id);
        Ok(found.map(|w| Target {
            frame_points: w.frame_points,
            window_title: w.window_title,
            ..target.clone()
        }))
    }

    pub async fn capture(
        &self,
        target: &Target,
        mode: &str,
    ) -> Result<(Observation, Option<Image>)> {
        if !self.rt.config().browser_use && is_browser(&target.app, target.bundle_id.as_deref()) {
            return Err(Error::msg(browser_refused(&target.app)));
        }
        if target.page_id.is_some() {
            let (target, root, image) = page::look(target, mode, self.cancel).await?;
            return Ok((self.save(&target, root, image.as_ref()), image));
        }
        let backend = self.desktop().await?;
        let mut root = backend.outline(target, 24, 1500).await?;
        root.assign_refs(&mut 0);
        let linux = matches!(backend.kind, Kind::Linux { .. });
        let image = if mode != "semantic" || linux {
            Some(backend.capture(target).await?)
        } else {
            None
        };
        Ok((self.save(target, root, image.as_ref()), image))
    }

    pub fn save(
        &self,
        target: &Target,
        root: crate::outline::Node,
        image: Option<&Image>,
    ) -> Observation {
        let mut store = self.rt.store();
        let state_id = store.new_state_id();
        let (width, height) = image.map_or((0, 0), |i| (i.width, i.height));
        let scale = if width > 0 && target.frame_points.w > 0.0 {
            f64::from(width) / target.frame_points.w
        } else {
            1.0
        };
        let observation = Observation {
            target: target.clone(),
            capture: Capture {
                state_id: state_id.clone(),
                width,
                height,
                scale_factor: scale,
                timestamp: now_ms(),
                coordinate_space: "window-relative-screenshot-pixels".into(),
            },
            outline: Outline {
                look_id: format!("look-{state_id}"),
                root,
            },
        };
        store.put_state(observation.clone());
        observation
    }
}
