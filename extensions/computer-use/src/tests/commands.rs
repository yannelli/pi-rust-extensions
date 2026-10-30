use std::path::Path;

use serde_json::json;

use crate::outline::Rect;
use crate::platform::linux::{self, Capturer};
use crate::platform::run::helper_result;
use crate::platform::{Cmd, Step, expand_keys, image_to_screen, jpeg_size, macos};

fn argv(cmd: &Cmd) -> Vec<&str> {
    std::iter::once(cmd.program.as_str())
        .chain(cmd.args.iter().map(String::as_str))
        .collect()
}

#[test]
fn macos_capture_and_convert() {
    let shot = macos::screencapture(81, Path::new("/tmp/a.png"));
    assert_eq!(
        argv(&shot),
        [
            "screencapture",
            "-x",
            "-o",
            "-l81",
            "-t",
            "png",
            "/tmp/a.png"
        ]
    );
    let jpeg = macos::to_jpeg(Path::new("/tmp/a.png"), Path::new("/tmp/a.jpg"), 1280);
    assert_eq!(
        argv(&jpeg),
        [
            "sips",
            "-Z",
            "1280",
            "-s",
            "format",
            "jpeg",
            "-s",
            "formatOptions",
            "70",
            "/tmp/a.png",
            "--out",
            "/tmp/a.jpg"
        ]
    );
}

#[test]
fn macos_helper_build_and_call() {
    let dir = macos::helper_dir(Path::new("/state"));
    assert!(
        dir.starts_with("/state")
            && dir
                .file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .starts_with("helper-")
    );
    let compile = macos::compile(Path::new("/h"));
    assert_eq!(
        argv(&compile),
        [
            "xcrun",
            "swiftc",
            "-O",
            "-o",
            "/h/pi-computer-use-helper",
            "/h/main.swift",
            "/h/ax.swift",
            "/h/input.swift"
        ]
    );
    let call = macos::helper(Path::new("/h/bin"), "outline", &json!({ "pid": 3 }));
    assert_eq!(argv(&call), ["/h/bin", "outline", "{\"pid\":3}"]);
    let args = macos::act_args(
        &json!({ "pid": 3 }),
        &[
            Step::Click {
                x: 1.0,
                y: 2.0,
                button: "left".into(),
                count: 2,
            },
            Step::Release,
        ],
    );
    assert_eq!(
        args,
        json!({ "pid": 3, "steps": [{ "op": "click", "x": 1.0, "y": 2.0, "button": "left", "count": 2 }, { "op": "release" }] })
    );
}

#[test]
fn linux_window_queries() {
    assert_eq!(
        argv(&linux::search_windows()),
        ["xdotool", "search", "--onlyvisible", "--name", ""]
    );
    assert_eq!(
        argv(&linux::window_geometry(9)),
        ["xdotool", "getwindowgeometry", "--shell", "9"]
    );
    let rect = linux::parse_geometry("WINDOW=9\nX=10\nY=20\nWIDTH=300\nHEIGHT=200\nSCREEN=0\n");
    assert_eq!(
        rect,
        Some(Rect {
            x: 10.0,
            y: 20.0,
            w: 300.0,
            h: 200.0
        })
    );
    assert_eq!(linux::parse_geometry("X=1"), None);
}

#[test]
fn linux_capture_choice() {
    let has = |list: &'static [&'static str]| move |p: &str| list.contains(&p);
    assert_eq!(
        linux::pick_capturer(true, has(&["grim", "import"])),
        Some(Capturer::Grim)
    );
    assert_eq!(
        linux::pick_capturer(false, has(&["grim", "import"])),
        Some(Capturer::Import)
    );
    assert_eq!(
        linux::pick_capturer(false, has(&["maim"])),
        Some(Capturer::Maim)
    );
    assert_eq!(linux::pick_capturer(true, has(&[])), None);
    let frame = Rect {
        x: 5.0,
        y: 6.0,
        w: 70.0,
        h: 80.0,
    };
    let out = Path::new("/tmp/l.jpg");
    assert_eq!(
        argv(&linux::capture(Capturer::Grim, 4, &frame, out)),
        ["grim", "-g", "5,6 70x80", "-t", "jpeg", "/tmp/l.jpg"]
    );
    assert_eq!(
        argv(&linux::capture(Capturer::Maim, 4, &frame, out)),
        ["maim", "-i", "4", "-f", "jpg", "/tmp/l.jpg"]
    );
    assert_eq!(
        argv(&linux::capture(Capturer::Import, 4, &frame, out)),
        ["import", "-window", "4", "jpg:/tmp/l.jpg"]
    );
}

#[test]
fn linux_input_steps() {
    let one = |step: Step| {
        linux::step(4, &step)
            .unwrap()
            .iter()
            .map(|c| argv(c).join(" "))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        one(Step::Click {
            x: 10.4,
            y: 20.6,
            button: "right".into(),
            count: 2
        }),
        [
            "xdotool mousemove --sync 10 21",
            "xdotool click --repeat 2 3"
        ]
    );
    assert_eq!(
        one(Step::Drag {
            path: vec![(1.0, 2.0), (3.0, 4.0), (5.0, 6.0)]
        }),
        [
            "xdotool mousemove --sync 1 2 mousedown 1 mousemove --sync 3 4 mousemove --sync 5 6 mouseup 1"
        ]
    );
    assert_eq!(
        one(Step::Scroll {
            x: 0.0,
            y: 0.0,
            dx: 0.0,
            dy: -100.0
        }),
        ["xdotool mousemove --sync 0 0", "xdotool click --repeat 3 4"]
    );
    assert_eq!(
        one(Step::Type { text: "-x".into() }),
        ["xdotool type --delay 8 -- -x"]
    );
    assert_eq!(
        one(Step::Keys {
            keys: vec!["cmd".into(), "a".into()]
        }),
        ["xdotool key --delay 8 -- super+a"]
    );
    assert_eq!(
        one(Step::Keys {
            keys: vec!["enter".into(), "f5".into()]
        }),
        ["xdotool key --delay 8 -- Return F5"]
    );
    assert_eq!(one(Step::Activate), ["xdotool windowactivate --sync 4"]);
    assert_eq!(
        one(Step::Release),
        ["xdotool mouseup 1 keyup super ctrl alt shift"]
    );
    assert!(linux::step(4, &Step::Press { path: vec![0] }).is_err());
}

#[test]
fn shared_helpers() {
    let frame = Rect {
        x: 100.0,
        y: 50.0,
        w: 800.0,
        h: 600.0,
    };
    assert_eq!(
        image_to_screen(&frame, 1600, 1200, 800.0, 600.0),
        (500.0, 350.0)
    );
    assert_eq!(
        expand_keys(&["Cmd+Shift+T".into(), " Enter ".into(), "+".into()]),
        ["cmd", "shift", "t", "enter", "+"]
    );
    let jpeg = [
        0xFF, 0xD8, 0xFF, 0xE0, 0, 4, 0, 0, 0xFF, 0xC0, 0, 17, 8, 0, 200, 1, 44, 3, 0, 0, 0, 0,
    ];
    assert_eq!(jpeg_size(&jpeg), Some((300, 200)));
    assert_eq!(jpeg_size(b"not a jpeg"), None);
    assert_eq!(
        helper_result("{\"ok\":true,\"result\":{\"a\":1}}\n").unwrap(),
        json!({ "a": 1 })
    );
    assert_eq!(
        helper_result("{\"ok\":false,\"error\":\"nope\"}")
            .unwrap_err()
            .message(),
        "nope"
    );
}
