pub mod linux;
pub mod macos;
pub mod run;

use crate::outline::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Mac,
    Linux { wayland: bool },
    Unsupported(&'static str),
}

pub fn detect() -> Os {
    match std::env::consts::OS {
        "macos" => Os::Mac,
        "linux" => Os::Linux {
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
        },
        other => Os::Unsupported(other),
    }
}

pub fn unsupported_message(os: &str) -> String {
    format!(
        "pi-computer-use does not support {os} in pi-rust. Supported platforms: macOS (screencapture and a Swift helper) and Linux (xdotool, with grim, maim, or ImageMagick import for screenshots)."
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
}

impl Cmd {
    pub fn new<S: AsRef<str>>(program: &str, args: &[S]) -> Self {
        Self {
            program: program.to_owned(),
            args: args.iter().map(|a| a.as_ref().to_owned()).collect(),
        }
    }
}

/// One low-level input step in screen points, shared by both backends.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Press {
        path: Vec<usize>,
    },
    SetText {
        path: Vec<usize>,
        text: String,
    },
    Focus {
        path: Vec<usize>,
    },
    Click {
        x: f64,
        y: f64,
        button: String,
        count: u32,
    },
    Move {
        x: f64,
        y: f64,
    },
    Drag {
        path: Vec<(f64, f64)>,
    },
    Scroll {
        x: f64,
        y: f64,
        dx: f64,
        dy: f64,
    },
    Type {
        text: String,
    },
    Keys {
        keys: Vec<String>,
    },
    Activate,
    Release,
}

impl Step {
    pub fn is_semantic(&self) -> bool {
        matches!(
            self,
            Self::Press { .. } | Self::SetText { .. } | Self::Focus { .. }
        )
    }
}

/// Maps a point in window screenshot pixels to global screen points.
pub fn image_to_screen(frame: &Rect, width: u32, height: u32, x: f64, y: f64) -> (f64, f64) {
    let sx = if width == 0 {
        1.0
    } else {
        frame.w / f64::from(width)
    };
    let sy = if height == 0 {
        1.0
    } else {
        frame.h / f64::from(height)
    };
    (frame.x + x * sx, frame.y + y * sy)
}

/// Splits `cmd+a` style keys and lowercases them.
pub fn expand_keys(keys: &[String]) -> Vec<String> {
    keys.iter()
        .flat_map(|key| {
            let lower = key.trim().to_lowercase();
            if lower.len() > 1 && lower.contains('+') {
                lower.split('+').map(str::to_owned).collect()
            } else {
                vec![lower]
            }
        })
        .filter(|key| !key.is_empty())
        .collect()
}

/// Reads width and height from a JPEG SOF marker.
pub fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.get(..2)? != [0xFF, 0xD8] {
        return None;
    }
    let mut at = 2;
    while at + 9 < bytes.len() {
        if bytes[at] != 0xFF {
            return None;
        }
        let marker = bytes[at + 1];
        let length = usize::from(u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]));
        if matches!(marker, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            let height = u16::from_be_bytes([bytes[at + 5], bytes[at + 6]]);
            let width = u16::from_be_bytes([bytes[at + 7], bytes[at + 8]]);
            return Some((u32::from(width), u32::from(height)));
        }
        at += 2 + length;
    }
    None
}
