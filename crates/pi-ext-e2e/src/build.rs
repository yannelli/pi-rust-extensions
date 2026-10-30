//! Paths of the pi binary and of workspace binaries the tests start.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

/// The default `PI_BIN`: a debug build of pi-rust `feat/extensions-platform`.
pub const DEFAULT_PI_BIN: &str = "/Users/ryanyannelli/Development/pi-upstream-ref/bin/pi-dev";

/// `PI_BIN`, or [`DEFAULT_PI_BIN`].
pub fn pi_bin() -> PathBuf {
    std::env::var_os("PI_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_PI_BIN))
}

/// The pi binary, or `None` after printing a skip message when it does not exist.
pub fn pi_bin_or_skip() -> Option<PathBuf> {
    let bin = pi_bin();
    if bin.is_file() {
        return Some(bin);
    }
    eprintln!(
        "skipping: pi binary {} does not exist; set PI_BIN",
        bin.display()
    );
    None
}

/// Run `cargo build -p <package> <target>` and return the executable cargo reports.
fn build(package: &str, target: &[&str]) -> PathBuf {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "build",
            "--message-format=json-render-diagnostics",
            "-p",
            package,
        ])
        .args(target)
        .current_dir(workspace_root())
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo build failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find_map(|message| message["executable"].as_str().map(PathBuf::from))
        .expect("cargo reported an executable")
}

/// Build the example `name` of `package` (for example `pi-ext-sdk`, `hello`).
pub fn cargo_example(package: &str, name: &str) -> PathBuf {
    build(package, &["--example", name])
}

/// Build the binary `name` of `package` (for example `pi-rust-ext`, `pi-rust-ext`).
pub fn cargo_bin(package: &str, name: &str) -> PathBuf {
    build(package, &["--bin", name])
}

fn workspace_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<name> layout")
}
