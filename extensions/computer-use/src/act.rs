use serde::Deserialize;

use crate::outline::Node;
use crate::platform::{Step, expand_keys, image_to_screen};
use crate::state::Observation;

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiAction {
    pub action: String,
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub x: Option<f64>,
    pub y: Option<f64>,
    pub button: Option<String>,
    pub click_count: Option<f64>,
    pub text: Option<String>,
    pub keys: Option<Vec<String>>,
    pub scroll_x: Option<f64>,
    pub scroll_y: Option<f64>,
    pub path: Option<Vec<Point>>,
}

pub struct Planner<'a> {
    pub observation: &'a Observation,
    pub semantic_tree: bool,
    pub headless: bool,
    pub mac: bool,
}

pub fn stale_ref(reference: &str) -> String {
    format!(
        "Outline ref '{reference}' is stale or not available for the latest state. Call observe_ui again and choose a current @e ref."
    )
}

impl Planner<'_> {
    fn node(&self, reference: &str) -> Result<&Node, String> {
        self.observation
            .outline
            .root
            .find(reference.trim())
            .ok_or_else(|| stale_ref(reference))
    }

    fn center(&self, node: &Node) -> Result<(f64, f64), String> {
        let frame = node.frame.ok_or_else(|| {
            format!(
                "Outline ref '{}' has no coordinates. Observe again or use a semantic action.",
                node.reference
            )
        })?;
        Ok(frame.center())
    }

    fn image_point(&self, x: f64, y: f64, label: &str) -> Result<(f64, f64), String> {
        let capture = &self.observation.capture;
        if capture.width == 0 || capture.height == 0 {
            return Err("Coordinate actions require an image-bearing state. Call observe_ui with mode fused or visual.".into());
        }
        if !x.is_finite() || !y.is_finite() {
            return Err(format!("{label} must be finite numbers."));
        }
        if x < 0.0 || y < 0.0 || x > f64::from(capture.width) || y > f64::from(capture.height) {
            return Err(format!(
                "{label} ({},{}) are outside the latest look image bounds ({}x{}). Call observe_ui again and retry.",
                x.round(),
                y.round(),
                capture.width,
                capture.height
            ));
        }
        let frame = &self.observation.target.frame_points;
        Ok(image_to_screen(frame, capture.width, capture.height, x, y))
    }

    fn semantic(&self, node: &Node, flag: bool) -> bool {
        self.semantic_tree && flag && !node.picture_only
    }

    fn focus(&self, reference: &str) -> Result<Step, String> {
        let node = self.node(reference)?;
        if self.semantic(node, node.can_focus) {
            return Ok(Step::Focus {
                path: node.path.clone(),
            });
        }
        let (x, y) = self.center(node)?;
        Ok(Step::Click {
            x,
            y,
            button: "left".into(),
            count: 1,
        })
    }

    fn one(&self, action: &UiAction, out: &mut Vec<Step>) -> Result<(), String> {
        let reference = action
            .reference
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty());
        let button = action.button.clone().unwrap_or_else(|| "left".into());
        let count = action.click_count.unwrap_or(1.0).round().clamp(1.0, 3.0) as u32;
        match action.action.as_str() {
            "press" | "click" => {
                let (x, y) = match (reference, action.x, action.y) {
                    (Some(reference), _, _) => {
                        let node = self.node(reference)?;
                        let plain = action.action == "press" || node.frame.is_none();
                        if plain && self.semantic(node, node.can_press) {
                            out.push(Step::Press {
                                path: node.path.clone(),
                            });
                            return Ok(());
                        }
                        self.center(node)?
                    }
                    (None, Some(x), Some(y)) => self.image_point(x, y, "Click coordinates")?,
                    _ => {
                        return Err(format!(
                            "{} requires either ref or both x and y.",
                            action.action
                        ));
                    }
                };
                out.push(Step::Click {
                    x,
                    y,
                    button,
                    count,
                });
            }
            "setText" => {
                let reference = reference.ok_or("setText requires an editable ref.")?;
                let node = self.node(reference)?;
                let text = action.text.clone().unwrap_or_default();
                if self.semantic(node, node.can_set_value) {
                    out.push(Step::SetText {
                        path: node.path.clone(),
                        text,
                    });
                } else {
                    let (x, y) = self.center(node)?;
                    let select = if self.mac { "cmd" } else { "ctrl" };
                    out.push(Step::Click {
                        x,
                        y,
                        button: "left".into(),
                        count: 1,
                    });
                    out.push(Step::Keys {
                        keys: vec![select.into(), "a".into()],
                    });
                    out.push(Step::Type { text });
                }
            }
            "typeText" | "keypress" => {
                if let Some(reference) = reference {
                    out.push(self.focus(reference)?);
                }
                if action.action == "typeText" {
                    out.push(Step::Type {
                        text: action.text.clone().unwrap_or_default(),
                    });
                } else {
                    let keys = expand_keys(action.keys.as_deref().unwrap_or_default());
                    if keys.is_empty() {
                        return Err("keypress.keys must contain at least one key.".into());
                    }
                    out.push(Step::Keys { keys });
                }
            }
            "scroll" => {
                let (x, y) = match reference {
                    Some(reference) => self.center(self.node(reference)?)?,
                    None => self.observation.target.frame_points.center(),
                };
                let clamp = |v: Option<f64>| v.unwrap_or(0.0).round().clamp(-10_000.0, 10_000.0);
                out.push(Step::Scroll {
                    x,
                    y,
                    dx: clamp(action.scroll_x),
                    dy: clamp(action.scroll_y),
                });
            }
            "drag" => {
                let points = action.path.as_deref().unwrap_or_default();
                if points.len() < 2 {
                    return Err("drag.path must contain at least two points.".into());
                }
                let mut path = Vec::new();
                for (index, p) in points.iter().enumerate() {
                    path.push(self.image_point(p.x, p.y, &format!("Drag point {}", index + 1))?);
                }
                out.push(Step::Drag { path });
            }
            "moveMouse" => {
                let (Some(x), Some(y)) = (action.x, action.y) else {
                    return Err("moveMouse requires x and y.".into());
                };
                let (x, y) = self.image_point(x, y, "Mouse coordinates")?;
                out.push(Step::Move { x, y });
            }
            other => return Err(format!("Unknown action '{other}'.")),
        }
        Ok(())
    }

    /// Plans every action first so a bad ref fails before any input is sent.
    pub fn plan(&self, actions: &[UiAction]) -> Result<Vec<Step>, String> {
        if actions.is_empty() || actions.len() > 20 {
            return Err("act_ui.actions must contain 1 to 20 actions.".into());
        }
        let mut steps = Vec::new();
        for action in actions {
            self.one(action, &mut steps)?;
        }
        let needs_input = steps.iter().any(|step| !step.is_semantic());
        if needs_input && self.headless {
            return Err("headless is enabled, so act_ui accepts only press and setText on accessibility refs.".into());
        }
        if needs_input {
            steps.insert(0, Step::Activate);
        }
        Ok(steps)
    }
}
