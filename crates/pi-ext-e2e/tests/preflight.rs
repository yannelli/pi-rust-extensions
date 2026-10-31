//! `pi_bin_or_skip` in a child copy of this test binary with a cleared environment, so each case
//! sees exactly the variables it sets and the parent's `PI_BIN` and `PI_REQUIRE_E2E` stay untouched.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use pi_ext_e2e::{DEFAULT_PI_BIN, TempDir, pi_bin_or_skip};

const CHILD: &str = "PI_E2E_PREFLIGHT_CHILD";

/// Runs only inside a child started by the cases below.
#[test]
fn child_runs_the_preflight() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    match pi_bin_or_skip() {
        Some(bin) => println!("child: some {}", bin.display()),
        None => println!("child: none"),
    }
}

fn child(env: &[(&str, &Path)]) -> Output {
    let exe = std::env::current_exe().unwrap();
    let mut command = Command::new(exe);
    command
        .args([
            "--exact",
            "child_runs_the_preflight",
            "--nocapture",
            "--test-threads=1",
        ])
        .env_clear()
        .env(CHILD, "1");
    for (key, value) in env {
        command.env(key, value);
    }
    command.output().unwrap()
}

fn text(output: &Output) -> (String, String) {
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    (stdout, stderr)
}

fn require() -> (&'static str, &'static Path) {
    ("PI_REQUIRE_E2E", Path::new("1"))
}

#[test]
fn required_without_pi_bin_fails_closed() {
    let output = child(&[require()]);
    let (stdout, stderr) = text(&output);
    assert!(!output.status.success(), "{stdout}{stderr}");
    assert!(stderr.contains("PI_REQUIRE_E2E=1 needs PI_BIN"), "{stderr}");
    assert!(!stdout.contains("child: none"), "{stdout}");
}

#[test]
fn a_set_pi_bin_that_does_not_exist_fails_without_require() {
    let dir = TempDir::new().unwrap();
    let missing = dir.path().join("no-pi");
    let output = child(&[("PI_BIN", missing.as_path())]);
    let (stdout, stderr) = text(&output);
    assert!(!output.status.success(), "{stdout}{stderr}");
    assert!(stderr.contains("is not an executable file"), "{stderr}");
}

#[cfg(unix)]
#[test]
fn a_set_pi_bin_without_the_executable_bit_fails() {
    let dir = TempDir::new().unwrap();
    let plain = dir.path().join("pi");
    std::fs::write(&plain, "not a binary").unwrap();
    let output = child(&[require(), ("PI_BIN", plain.as_path())]);
    let (stdout, stderr) = text(&output);
    assert!(!output.status.success(), "{stdout}{stderr}");
    assert!(stderr.contains("is not an executable file"), "{stderr}");
}

#[test]
fn required_with_an_executable_pi_bin_uses_it() {
    let exe = std::env::current_exe().unwrap();
    let output = child(&[require(), ("PI_BIN", exe.as_path())]);
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains(&format!("child: some {}", exe.display())),
        "{stdout}"
    );
}

#[test]
fn without_pi_bin_or_require_the_default_is_used_or_skipped_with_a_message() {
    let output = child(&[]);
    let (stdout, stderr) = text(&output);
    assert!(output.status.success(), "{stdout}{stderr}");
    let default = PathBuf::from(DEFAULT_PI_BIN);
    if executable(&default) {
        assert!(
            stdout.contains(&format!("child: some {}", default.display())),
            "{stdout}"
        );
    } else {
        assert!(stdout.contains("child: none"), "{stdout}");
        assert!(stderr.contains("skipping: pi binary"), "{stderr}");
    }
}

fn executable(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}
