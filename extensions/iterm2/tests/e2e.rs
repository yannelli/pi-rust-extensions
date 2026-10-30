//! Under the real pi binary in iTerm2's environment: JSON mode has no TUI, so the extension
//! stays quiet and the run completes. Escape sequences are covered by `protocol.rs`.

use pi_ext_e2e::{Run, Turn, cargo_bin, pi_bin_or_skip};

#[test]
fn iterm2_stays_quiet_without_a_tui() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["iterm2"])
        .env("TERM_PROGRAM", "iTerm.app")
        .turns([Turn::text("done")])
        .prompt("Hello")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(output.final_text(), "done");
    assert!(!output.stdout.contains("\x1b]1337"), "{}", output.stdout);
}
