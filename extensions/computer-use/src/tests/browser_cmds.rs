use std::path::Path;

use serde_json::json;

use crate::browser::{check_url, executable, launch_args, pick_port};
use crate::config::ManagedBrowser;
use crate::page::page_roots;
use crate::page_act::{key_event, modifier_bit};
use crate::platform::macos;

#[test]
fn browser_launch_and_urls() {
    let args = launch_args(9333, Path::new("/state/browser-chrome"));
    assert_eq!(
        args,
        [
            "--remote-debugging-port=9333",
            "--user-data-dir=/state/browser-chrome",
            "--no-first-run",
            "--no-default-browser-check",
            "about:blank"
        ]
    );
    assert!(check_url("https://example.com").is_ok() && check_url("about:blank").is_ok());
    assert!(check_url("file:///etc/passwd").is_err() && check_url("javascript:alert(1)").is_err());
    let env =
        |key: &str| (key == "PI_COMPUTER_USE_HELIUM_EXECUTABLE").then(|| "/opt/helium".to_owned());
    assert_eq!(
        executable(ManagedBrowser::Helium, env, |_| false).unwrap(),
        Path::new("/opt/helium")
    );
    assert_eq!(
        pick_port(|key| (key == "PI_COMPUTER_USE_CDP_PORT").then(|| "9555".into())).unwrap(),
        9555
    );
    assert!(pick_port(|_| None).unwrap() > 0);
}

#[test]
fn browser_keys_and_pages() {
    let enter = key_event("enter").unwrap();
    assert_eq!(
        (enter.key.as_str(), enter.vk, enter.text.as_deref()),
        ("Enter", 13, Some("\r"))
    );
    let a = key_event("a").unwrap();
    assert_eq!((a.code.as_str(), a.vk), ("KeyA", 65));
    assert_eq!(key_event("f2").unwrap().vk, 113);
    assert!(key_event("hyper").is_none());
    assert_eq!(
        [
            modifier_bit("alt"),
            modifier_bit("ctrl"),
            modifier_bit("cmd"),
            modifier_bit("shift")
        ],
        [Some(1), Some(2), Some(4), Some(8)]
    );
    let list = json!([
        { "id": "P1", "type": "page", "title": "Home", "url": "https://a.test/" },
        { "id": "W1", "type": "service_worker", "title": "sw", "url": "" }
    ]);
    let roots = page_roots(&list, "Chrome", 77);
    assert_eq!(roots.len(), 1);
    assert_eq!(
        (roots[0].page_id.as_deref(), roots[0].kind.as_deref()),
        (Some("P1"), Some("browser_page"))
    );
    assert_eq!(roots[0].app, "Chrome (managed)");
}

#[cfg(target_os = "macos")]
#[test]
fn swift_helper_typechecks() {
    if !crate::platform::run::which("xcrun") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut args = vec!["swiftc".to_owned(), "-typecheck".to_owned()];
    for (name, text) in macos::SOURCES {
        std::fs::write(dir.path().join(name), text).unwrap();
        args.push(dir.path().join(name).display().to_string());
    }
    let output = std::process::Command::new("/usr/bin/timeout")
        .arg("600")
        .arg("xcrun")
        .args(&args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
