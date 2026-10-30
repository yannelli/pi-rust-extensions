//! Under the real pi binary: the extension loads and follows the events of a run. JSON mode
//! has no footer, so footer lines are covered by `protocol.rs`.

use pi_ext_e2e::{Run, Turn, cargo_bin, pi_bin_or_skip};

#[test]
fn statusline_loads_and_follows_a_run() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["statusline"])
        .turns([Turn::text("done")])
        .prompt("Hello")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(output.final_text(), "done");
    assert!(
        !output.stderr.contains("statusline: footer update failed"),
        "{}",
        output.stderr
    );
}
