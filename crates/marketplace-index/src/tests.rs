use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Value, json};

use crate::archives::{self, Archive, CHECKSUMS, TARGETS};
use crate::catalog::{build_index, is_extension_name, is_version, render};

const DIGEST: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("marketplace-index-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn sample_archives() -> Vec<Archive> {
    TARGETS
        .iter()
        .map(|target| Archive {
            target,
            file_name: archives::archive_name(target),
            sha256: DIGEST.to_string(),
            size: 10,
        })
        .collect()
}

fn sample_catalog() -> Value {
    json!({
        "schemaVersion": 1,
        "minPiVersion": "0.8.0",
        "extensions": [
            { "name": "todo", "displayName": "Todo", "description": "Tracks tasks.", "tags": ["tui"] }
        ]
    })
}

fn build_error(catalog: &Value) -> String {
    build_index(catalog, "v0.1.0", "0.1.0", &sample_archives()).unwrap_err()
}

#[test]
fn builds_contract_index() {
    let index = build_index(&sample_catalog(), "v0.1.0", "0.1.0", &sample_archives()).unwrap();
    let base = "https://github.com/yannelli/pi-rust-extensions/releases/download/v0.1.0";
    assert_eq!(index["schemaVersion"], 1);
    assert_eq!(index["repository"], "yannelli/pi-rust-extensions");
    assert_eq!(index["tag"], "v0.1.0");
    assert_eq!(index["protocolVersion"], 1);
    assert_eq!(index["minPiVersion"], "0.8.0");
    let binaries = index["binaries"]["pi-rust-ext"].as_object().unwrap();
    assert_eq!(binaries.len(), 5);
    assert_eq!(
        binaries["x86_64-pc-windows-msvc"],
        json!({ "url": format!("{base}/pi-rust-ext-x86_64-pc-windows-msvc.zip"), "sha256": DIGEST, "size": 10 })
    );
    assert_eq!(
        binaries["aarch64-apple-darwin"]["url"],
        format!("{base}/pi-rust-ext-aarch64-apple-darwin.tar.gz")
    );
    assert_eq!(
        index["extensions"][0],
        json!({
            "name": "todo", "displayName": "Todo", "description": "Tracks tasks.", "version": "0.1.0",
            "binary": "pi-rust-ext", "args": ["todo"], "tags": ["tui"], "platforms": TARGETS,
        })
    );
    let keys: Vec<&str> = index
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "schemaVersion",
            "repository",
            "tag",
            "protocolVersion",
            "minPiVersion",
            "binaries",
            "extensions"
        ]
    );
}

#[test]
fn defaults_min_pi_version_and_renders_trailing_newline() {
    let catalog = json!({ "schemaVersion": 1, "extensions": [] });
    let index = build_index(&catalog, "v0.1.0", "0.1.0", &sample_archives()).unwrap();
    assert_eq!(index["minPiVersion"], "0.7.7");
    let text = render(&index);
    assert!(text.ends_with("}\n"));
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), index);
}

#[test]
fn rejects_tag_that_does_not_match_version() {
    let archives = sample_archives();
    assert!(build_index(&sample_catalog(), "v0.2.0", "0.1.0", &archives).is_err());
    assert!(build_index(&sample_catalog(), "0.1.0", "0.1.0", &archives).is_err());
    assert!(build_index(&sample_catalog(), "v0.1.0/../x", "0.1.0/../x", &archives).is_err());
}

#[test]
fn rejects_invalid_catalogs() {
    let cases = [
        (
            json!({ "schemaVersion": 2, "extensions": [] }),
            "schemaVersion",
        ),
        (json!({ "schemaVersion": 1 }), "extensions must be an array"),
        (
            json!({ "schemaVersion": 1, "extensions": [], "extra": 1 }),
            "unknown field",
        ),
        (
            json!({ "schemaVersion": 1, "minPiVersion": "latest", "extensions": [] }),
            "minPiVersion",
        ),
        (
            json!({ "schemaVersion": 1, "protocolVersion": 0, "extensions": [] }),
            "protocolVersion",
        ),
    ];
    for (catalog, expected) in cases {
        let error = build_error(&catalog);
        assert!(error.contains(expected), "{error}");
    }
}

#[test]
fn rejects_invalid_extensions() {
    let entry = |extra: Value| {
        let mut base = json!({ "name": "todo", "displayName": "Todo", "description": "d" });
        base.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        json!({ "schemaVersion": 1, "extensions": [base] })
    };
    let cases = [
        (
            entry(json!({ "name": "../todo" })),
            "invalid extension name",
        ),
        (entry(json!({ "name": "Todo" })), "invalid extension name"),
        (
            entry(json!({ "binary": "other" })),
            "unknown field \"binary\"",
        ),
        (
            entry(json!({ "platforms": [] })),
            "unknown field \"platforms\"",
        ),
        (
            entry(json!({ "displayName": "" })),
            "displayName is required",
        ),
        (entry(json!({ "args": ["other"] })), "args must start"),
        (entry(json!({ "tags": [1] })), "tags must be an array"),
        (entry(json!({ "version": "1.0" })), "invalid version"),
    ];
    for (catalog, expected) in cases {
        let error = build_error(&catalog);
        assert!(error.contains(expected), "{error}");
    }
    let todo = json!({ "name": "todo", "displayName": "Todo", "description": "d" });
    let duplicate = json!({ "schemaVersion": 1, "extensions": [todo.clone(), todo] });
    assert!(build_error(&duplicate).contains("duplicate extension"));
}

#[test]
fn validates_names_and_versions() {
    assert!(is_extension_name("todo"));
    assert!(is_extension_name("git-status2"));
    for bad in ["", "-a", "a-", "a/b", "a.b", "A", ".."] {
        assert!(!is_extension_name(bad), "{bad}");
    }
    assert!(is_version("0.1.0"));
    assert!(is_version("1.2.3-rc.1"));
    assert!(is_version("10.0.0-0.alpha-1.01a"));
    for bad in [
        "1.2",
        "1.2.3+build",
        "1.2.x",
        "1.2.3-",
        "1.2.3-a..b",
        "01.2.3",
        "1.02.3",
        "1.2.3-rc.01",
        "v1.2.3",
    ] {
        assert!(!is_version(bad), "{bad}");
    }
}

#[test]
fn parses_checksum_lines() {
    let text = format!("{DIGEST}  a.tar.gz\n{DIGEST} *b.zip\n\n");
    let sums = archives::parse_checksums(&text).unwrap();
    assert_eq!(sums["a.tar.gz"], DIGEST);
    assert_eq!(sums["b.zip"], DIGEST);
}

#[test]
fn rejects_bad_checksum_lines() {
    let upper = DIGEST.to_uppercase();
    let cases = [
        format!("{upper}  a.tar.gz"),
        format!("{}  a.tar.gz", &DIGEST[1..]),
        format!("{DIGEST}  ../a.tar.gz"),
        format!("{DIGEST}  dir/a.tar.gz"),
        format!("{DIGEST}  a\\b"),
        format!("{DIGEST}a.tar.gz"),
        format!("{DIGEST}  a.zip\n{DIGEST}  a.zip"),
    ];
    for text in cases {
        assert!(archives::parse_checksums(&text).is_err(), "{text}");
    }
}

#[test]
fn collects_archives_from_directory() {
    let dir = TempDir::new();
    let mut sums = String::new();
    for (i, target) in TARGETS.iter().enumerate() {
        let name = archives::archive_name(target);
        std::fs::write(dir.0.join(&name), vec![0u8; i + 1]).unwrap();
        sums.push_str(&format!("{DIGEST}  {name}\n"));
    }
    std::fs::write(dir.0.join(CHECKSUMS), &sums).unwrap();
    let found = archives::collect(&dir.0).unwrap();
    assert_eq!(found.len(), 5);
    assert_eq!(found[4].file_name, "pi-rust-ext-x86_64-pc-windows-msvc.zip");
    assert_eq!(found[4].size, 5);

    std::fs::remove_file(dir.0.join("pi-rust-ext-x86_64-apple-darwin.tar.gz")).unwrap();
    assert!(archives::collect(&dir.0).is_err());
}

#[test]
fn rejects_target_missing_from_checksums() {
    let dir = TempDir::new();
    let name = archives::archive_name(TARGETS[0]);
    std::fs::write(dir.0.join(&name), b"x").unwrap();
    std::fs::write(dir.0.join(CHECKSUMS), format!("{DIGEST}  {name}\n")).unwrap();
    let error = archives::collect(&dir.0).unwrap_err();
    assert!(
        error.contains("no entry for pi-rust-ext-x86_64-apple-darwin.tar.gz"),
        "{error}"
    );
}
