use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Cmd, Step};

pub const SOURCES: [(&str, &str); 3] = [
    ("main.swift", include_str!("../../swift/main.swift")),
    ("ax.swift", include_str!("../../swift/ax.swift")),
    ("input.swift", include_str!("../../swift/input.swift")),
];

pub const HELPER_NAME: &str = "pi-computer-use-helper";

pub fn source_hash() -> String {
    let mut hasher = DefaultHasher::new();
    for (name, text) in SOURCES {
        name.hash(&mut hasher);
        text.hash(&mut hasher);
    }
    format!("{:016x}", hasher.finish())
}

/// `<state>/helper-<hash>/`: a source change makes a new directory.
pub fn helper_dir(state_dir: &Path) -> PathBuf {
    state_dir.join(format!("helper-{}", source_hash()))
}

pub fn compile(dir: &Path) -> Cmd {
    let mut args = vec!["swiftc".to_owned(), "-O".to_owned(), "-o".to_owned()];
    args.push(dir.join(HELPER_NAME).display().to_string());
    args.extend(
        SOURCES
            .iter()
            .map(|(name, _)| dir.join(name).display().to_string()),
    );
    Cmd {
        program: "xcrun".into(),
        args,
    }
}

pub fn find_swiftc() -> Cmd {
    Cmd::new("xcrun", &["--find", "swiftc"])
}

pub fn install_tools() -> Cmd {
    Cmd::new("xcode-select", &["--install"])
}

pub fn helper(binary: &Path, command: &str, args: &Value) -> Cmd {
    Cmd {
        program: binary.display().to_string(),
        args: vec![command.to_owned(), args.to_string()],
    }
}

pub fn screencapture(window_id: i64, path: &Path) -> Cmd {
    let args = [
        "-x".to_owned(),
        "-o".to_owned(),
        format!("-l{window_id}"),
        "-t".to_owned(),
        "png".to_owned(),
        path.display().to_string(),
    ];
    Cmd::new("screencapture", &args)
}

pub fn to_jpeg(input: &Path, output: &Path, max_dimension: u32) -> Cmd {
    let args = [
        "-Z".to_owned(),
        max_dimension.to_string(),
        "-s".to_owned(),
        "format".to_owned(),
        "jpeg".to_owned(),
        "-s".to_owned(),
        "formatOptions".to_owned(),
        "70".to_owned(),
        input.display().to_string(),
        "--out".to_owned(),
        output.display().to_string(),
    ];
    Cmd::new("sips", &args)
}

pub fn step_json(step: &Step) -> Value {
    match step {
        Step::Press { path } => json!({ "op": "press", "path": path }),
        Step::SetText { path, text } => json!({ "op": "setText", "path": path, "text": text }),
        Step::Focus { path } => json!({ "op": "focus", "path": path }),
        Step::Click {
            x,
            y,
            button,
            count,
        } => {
            json!({ "op": "click", "x": x, "y": y, "button": button, "count": count })
        }
        Step::Move { x, y } => json!({ "op": "move", "x": x, "y": y }),
        Step::Drag { path } => {
            let points: Vec<Value> = path
                .iter()
                .map(|(x, y)| json!({ "x": x, "y": y }))
                .collect();
            json!({ "op": "drag", "path": points })
        }
        Step::Scroll { x, y, dx, dy } => {
            json!({ "op": "scroll", "x": x, "y": y, "dx": dx, "dy": dy })
        }
        Step::Type { text } => json!({ "op": "type", "text": text }),
        Step::Keys { keys } => json!({ "op": "keys", "keys": keys }),
        Step::Activate => json!({ "op": "activate" }),
        Step::Release => json!({ "op": "release" }),
    }
}

pub fn act_args(target: &Value, steps: &[Step]) -> Value {
    let mut args = target.clone();
    args["steps"] = Value::Array(steps.iter().map(step_json).collect());
    args
}
