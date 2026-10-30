use std::path::PathBuf;
use std::time::Duration;

use base64::Engine;
use pi_ext_sdk::{CancellationToken, Error, Result};
use serde_json::{Value, json};

use crate::outline::Node;
use crate::platform::linux::{self, Capturer};
use crate::platform::run::{helper_result, run, run_ok};
use crate::platform::{Cmd, Step, jpeg_size, macos};
use crate::state::{Root, Target};

const SHORT: Duration = Duration::from_secs(15);
const LONG: Duration = Duration::from_secs(45);
const RELEASE: Duration = Duration::from_secs(3);
pub const MAX_IMAGE: u32 = 1280;

pub struct Image {
    pub base64: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub enum Kind {
    MacOs { helper: PathBuf },
    Linux { capturer: Option<Capturer> },
}

#[derive(Debug, Clone)]
pub struct Backend {
    pub kind: Kind,
    pub scratch: PathBuf,
    pub cancel: CancellationToken,
}

fn target_json(target: &Target) -> Value {
    json!({ "pid": target.pid, "windowTitle": target.window_title, "frame": target.frame_points })
}

impl Backend {
    pub async fn helper(&self, command: &str, args: Value) -> Result<Value> {
        let Kind::MacOs { helper } = &self.kind else {
            return Err(Error::msg(
                "The macOS helper is not available on this platform.",
            ));
        };
        let output = run(&macos::helper(helper, command, &args), LONG, &self.cancel).await?;
        helper_result(&output.stdout)
    }

    async fn text(&self, cmd: Cmd) -> Result<String> {
        Ok(run_ok(&cmd, SHORT, &self.cancel).await?.trim().to_owned())
    }

    pub async fn windows(&self) -> Result<Vec<Root>> {
        if matches!(self.kind, Kind::MacOs { .. }) {
            let result = self.helper("windows", json!({})).await?;
            let windows = result["windows"].as_array().cloned().unwrap_or_default();
            return Ok(windows
                .into_iter()
                .filter_map(|w| serde_json::from_value(w).ok())
                .collect());
        }
        let active = self
            .text(linux::active_window())
            .await
            .ok()
            .and_then(|id| id.parse::<i64>().ok());
        let ids = self.text(linux::search_windows()).await.unwrap_or_default();
        let mut roots = Vec::new();
        for id in ids
            .lines()
            .filter_map(|line| line.trim().parse::<i64>().ok())
            .take(64)
        {
            let Some(frame) = linux::parse_geometry(&self.text(linux::window_geometry(id)).await?)
            else {
                continue;
            };
            if frame.w < 2.0 || frame.h < 2.0 {
                continue;
            }
            let pid = self
                .text(linux::window_pid(id))
                .await
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            roots.push(Root {
                app: linux::process_name(pid).unwrap_or_else(|| "Unknown App".into()),
                pid,
                window_title: self.text(linux::window_name(id)).await.unwrap_or_default(),
                window_id: id,
                frame_points: frame,
                scale_factor: 1.0,
                is_onscreen: true,
                is_focused: active == Some(id),
                ..Root::default()
            });
        }
        Ok(roots)
    }

    pub async fn outline(&self, target: &Target, max_depth: u32, max_nodes: u32) -> Result<Node> {
        if matches!(self.kind, Kind::Linux { .. }) {
            return Ok(picture_root(target));
        }
        let mut args = target_json(target);
        args["maxDepth"] = json!(max_depth);
        args["maxNodes"] = json!(max_nodes);
        let result = self.helper("outline", args).await?;
        let mut root: Node = serde_json::from_value(result["root"].clone())
            .map_err(|err| Error::msg(format!("Helper outline is invalid: {err}")))?;
        normalize(&mut root);
        Ok(root)
    }

    pub async fn capture(&self, target: &Target) -> Result<Image> {
        std::fs::create_dir_all(&self.scratch)?;
        let stem = self.scratch.join(format!(
            "look-{}-{}",
            std::process::id(),
            crate::state::now_ms()
        ));
        let jpeg = stem.with_extension("jpg");
        match &self.kind {
            Kind::MacOs { .. } => {
                let png = stem.with_extension("png");
                run_ok(
                    &macos::screencapture(target.window_id, &png),
                    SHORT,
                    &self.cancel,
                )
                .await?;
                let converted =
                    run_ok(&macos::to_jpeg(&png, &jpeg, MAX_IMAGE), SHORT, &self.cancel).await;
                let _ = std::fs::remove_file(&png);
                converted?;
            }
            Kind::Linux { capturer } => {
                let Some(capturer) = capturer else {
                    return Err(Error::msg(
                        "No screenshot tool found. Install grim (Wayland), maim, or ImageMagick.",
                    ));
                };
                let cmd = linux::capture(*capturer, target.window_id, &target.frame_points, &jpeg);
                run_ok(&cmd, SHORT, &self.cancel).await?;
            }
        }
        let bytes = std::fs::read(&jpeg);
        let _ = std::fs::remove_file(&jpeg);
        let bytes =
            bytes.map_err(|err| Error::msg(format!("Screenshot was not written: {err}")))?;
        let (width, height) =
            jpeg_size(&bytes).ok_or_else(|| Error::msg("Screenshot is not a JPEG image"))?;
        let base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        Ok(Image {
            base64,
            width,
            height,
        })
    }

    /// Runs the steps; returns one outcome object per attempted step.
    pub async fn act(&self, target: &Target, steps: &[Step]) -> Result<Vec<Value>> {
        let outcomes = self.act_steps(target, steps).await;
        let failed = outcomes
            .as_ref()
            .map_or(true, |list| list.iter().any(|o| o["outcome"] != "worked"));
        if failed {
            self.release().await;
        }
        outcomes
    }

    /// Lifts the mouse button and modifiers with a fresh token so cancel does not skip it.
    pub async fn release(&self) {
        let fresh = CancellationToken::new();
        let cmd = match &self.kind {
            Kind::MacOs { helper } => macos::helper(
                helper,
                "act",
                &macos::act_args(&json!({}), &[Step::Release]),
            ),
            Kind::Linux { .. } => linux::release(),
        };
        let _ = run(&cmd, RELEASE, &fresh).await;
    }

    async fn act_steps(&self, target: &Target, steps: &[Step]) -> Result<Vec<Value>> {
        if matches!(self.kind, Kind::MacOs { .. }) {
            let result = self
                .helper("act", macos::act_args(&target_json(target), steps))
                .await?;
            return Ok(result["steps"].as_array().cloned().unwrap_or_default());
        }
        let mut outcomes = Vec::new();
        for (index, step) in steps.iter().enumerate() {
            let cmds = match linux::step(target.window_id, step) {
                Ok(cmds) => cmds,
                Err(err) => {
                    outcomes.push(json!({ "index": index, "outcome": "didnt", "error": err }));
                    break;
                }
            };
            for cmd in &cmds {
                if let Err(err) = run_ok(cmd, SHORT, &self.cancel).await {
                    outcomes.push(
                        json!({ "index": index, "outcome": "didnt", "error": err.message() }),
                    );
                    return Ok(outcomes);
                }
            }
            outcomes.push(json!({ "index": index, "outcome": "worked" }));
        }
        Ok(outcomes)
    }
}

fn normalize(node: &mut Node) {
    node.role = crate::outline::normalize_role(&node.role);
    for child in &mut node.children {
        normalize(child);
    }
}

pub fn picture_root(target: &Target) -> Node {
    Node {
        role: "window".into(),
        label: Some(target.window_title.clone()).filter(|t| !t.is_empty()),
        frame: Some(target.frame_points),
        picture_only: true,
        ..Node::default()
    }
}
