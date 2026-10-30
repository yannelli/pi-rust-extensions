//! rewind under the pi binary: a turn that writes a file records a checkpoint with one changed file.

use pi_ext_e2e::{Run, Turn, cargo_bin, json, pi_bin_or_skip};

#[test]
fn turn_with_write_records_checkpoint() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["rewind"])
        .env("PI_EXT_LOG", "info")
        .turns([
            Turn::tool_call("write", json!({"path": "note.txt", "content": "hello\n"})),
            Turn::text("Wrote the note."),
        ])
        .prompt("Write a note")
        .run()
        .unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    assert_eq!(output.final_text(), "Wrote the note.");
    let line = output
        .stderr
        .lines()
        .find(|line| line.contains("checkpoint "))
        .unwrap_or_else(|| panic!("no checkpoint log line:\n{}", output.stderr));
    assert!(line.ends_with("files=1"), "{line}");
}
