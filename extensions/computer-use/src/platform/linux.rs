use std::path::Path;

use super::{Cmd, Step};
use crate::outline::Rect;

pub fn search_windows() -> Cmd {
    Cmd::new("xdotool", &["search", "--onlyvisible", "--name", ""])
}

pub fn active_window() -> Cmd {
    Cmd::new("xdotool", &["getactivewindow"])
}

pub fn window_name(id: i64) -> Cmd {
    Cmd::new("xdotool", &["getwindowname".to_owned(), id.to_string()])
}

pub fn window_pid(id: i64) -> Cmd {
    Cmd::new("xdotool", &["getwindowpid".to_owned(), id.to_string()])
}

pub fn window_geometry(id: i64) -> Cmd {
    Cmd::new(
        "xdotool",
        &[
            "getwindowgeometry".to_owned(),
            "--shell".to_owned(),
            id.to_string(),
        ],
    )
}

/// Parses `xdotool getwindowgeometry --shell` output.
pub fn parse_geometry(text: &str) -> Option<Rect> {
    let field = |name: &str| -> Option<f64> {
        text.lines().find_map(|line| {
            line.strip_prefix(name)?
                .strip_prefix('=')?
                .trim()
                .parse()
                .ok()
        })
    };
    Some(Rect {
        x: field("X")?,
        y: field("Y")?,
        w: field("WIDTH")?,
        h: field("HEIGHT")?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capturer {
    Grim,
    Maim,
    Import,
}

pub fn pick_capturer(wayland: bool, has: impl Fn(&str) -> bool) -> Option<Capturer> {
    let order: &[(Capturer, &str)] = if wayland {
        &[(Capturer::Grim, "grim"), (Capturer::Import, "import")]
    } else {
        &[
            (Capturer::Maim, "maim"),
            (Capturer::Import, "import"),
            (Capturer::Grim, "grim"),
        ]
    };
    order
        .iter()
        .find(|(_, program)| has(program))
        .map(|(capturer, _)| *capturer)
}

pub fn capture(capturer: Capturer, window_id: i64, frame: &Rect, path: &Path) -> Cmd {
    let out = path.display().to_string();
    match capturer {
        Capturer::Grim => {
            let geometry = format!("{},{} {}x{}", frame.x, frame.y, frame.w, frame.h);
            Cmd::new(
                "grim",
                &[
                    "-g".to_owned(),
                    geometry,
                    "-t".to_owned(),
                    "jpeg".to_owned(),
                    out,
                ],
            )
        }
        Capturer::Maim => Cmd::new(
            "maim",
            &[
                "-i".to_owned(),
                window_id.to_string(),
                "-f".to_owned(),
                "jpg".to_owned(),
                out,
            ],
        ),
        Capturer::Import => Cmd::new(
            "import",
            &[
                "-window".to_owned(),
                window_id.to_string(),
                format!("jpg:{out}"),
            ],
        ),
    }
}

pub fn key_name(key: &str) -> String {
    let mapped = match key {
        "cmd" | "command" | "meta" | "super" => "super",
        "ctrl" | "control" => "ctrl",
        "alt" | "option" | "opt" => "alt",
        "shift" => "shift",
        "enter" | "return" => "Return",
        "escape" | "esc" => "Escape",
        "backspace" => "BackSpace",
        "delete" | "forwarddelete" => "Delete",
        "tab" => "Tab",
        "space" => "space",
        "up" | "arrowup" => "Up",
        "down" | "arrowdown" => "Down",
        "left" | "arrowleft" => "Left",
        "right" | "arrowright" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" => "Prior",
        "pagedown" => "Next",
        other if other.len() > 1 && other.starts_with('f') && other[1..].parse::<u8>().is_ok() => {
            return other.to_uppercase();
        }
        other => other,
    };
    mapped.to_owned()
}

fn is_modifier(key: &str) -> bool {
    matches!(key, "super" | "ctrl" | "alt" | "shift")
}

/// Chords join with `+`; plain key lists press keys in order.
pub fn key_args(keys: &[String]) -> Vec<String> {
    let names: Vec<String> = keys.iter().map(|k| key_name(k)).collect();
    let chord = names.len() >= 2 && names[..names.len() - 1].iter().all(|k| is_modifier(k));
    if chord { vec![names.join("+")] } else { names }
}

fn button(name: &str) -> &'static str {
    match name {
        "right" => "3",
        "middle" => "2",
        _ => "1",
    }
}

fn px(value: f64) -> String {
    format!("{}", value.round() as i64)
}

/// Builds the xdotool invocations for one step against `window_id`.
pub fn step(window_id: i64, step: &Step) -> Result<Vec<Cmd>, String> {
    let x = |args: Vec<String>| Cmd {
        program: "xdotool".into(),
        args,
    };
    let s = |v: &[&str]| v.iter().map(|a| (*a).to_owned()).collect::<Vec<_>>();
    let mv = |px_: f64, py: f64| x(vec!["mousemove".into(), "--sync".into(), px(px_), px(py)]);
    Ok(match step {
        Step::Press { .. } | Step::SetText { .. } | Step::Focus { .. } => {
            return Err(
                "Linux has no accessibility tree in pi-rust; use coordinate actions.".into(),
            );
        }
        Step::Release => vec![release()],
        Step::Activate => vec![x(vec![
            "windowactivate".into(),
            "--sync".into(),
            window_id.to_string(),
        ])],
        Step::Move { x: px_, y } => vec![mv(*px_, *y)],
        Step::Click {
            x: px_,
            y,
            button: b,
            count,
        } => {
            let click = x(vec![
                "click".into(),
                "--repeat".into(),
                count.to_string(),
                button(b).into(),
            ]);
            vec![mv(*px_, *y), click]
        }
        Step::Drag { path } => {
            let mut args = Vec::new();
            for (index, (px_, py)) in path.iter().enumerate() {
                args.extend(["mousemove".into(), "--sync".into(), px(*px_), px(*py)]);
                if index == 0 {
                    args.extend(s(&["mousedown", "1"]));
                }
            }
            args.extend(s(&["mouseup", "1"]));
            vec![x(args)]
        }
        Step::Scroll { x: px_, y, dx, dy } => {
            let mut cmds = vec![mv(*px_, *y)];
            for (delta, neg, pos) in [(*dy, "4", "5"), (*dx, "6", "7")] {
                let clicks = (delta.abs() / 40.0).ceil() as i64;
                if clicks > 0 {
                    let b = if delta < 0.0 { neg } else { pos };
                    cmds.push(x(vec![
                        "click".into(),
                        "--repeat".into(),
                        clicks.to_string(),
                        b.into(),
                    ]));
                }
            }
            cmds
        }
        Step::Type { text } => vec![x(vec![
            "type".into(),
            "--delay".into(),
            "8".into(),
            "--".into(),
            text.clone(),
        ])],
        Step::Keys { keys } => {
            let mut args = s(&["key", "--delay", "8", "--"]);
            args.extend(key_args(keys));
            vec![x(args)]
        }
    })
}

/// Releases the left button and modifiers after a failed or cancelled step.
pub fn release() -> Cmd {
    Cmd::new(
        "xdotool",
        &["mouseup", "1", "keyup", "super", "ctrl", "alt", "shift"],
    )
}

pub fn process_name(pid: i64) -> Option<String> {
    let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    Some(comm.trim().to_owned()).filter(|name| !name.is_empty())
}
