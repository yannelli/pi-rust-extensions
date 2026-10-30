use std::os::unix::fs::PermissionsExt;

use pi_ext_e2e::{Run, Turn, cargo_bin, pi_bin, pi_bin_or_skip};

const MARKER: &str = "CONTEXT-INCLUDE-MARKER-7f3a";

#[test]
fn notes_reference_reaches_system_prompt() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let wrapper = dir.path().join("pi");
    let script = format!(
        "#!/bin/sh\nprintf '# Rules\\n@notes.md\\n' > \"$PWD/AGENTS.md\"\n\
         printf '{MARKER}\\n' > \"$PWD/notes.md\"\nexec '{}' \"$@\"\n",
        pi_bin().display()
    );
    std::fs::write(&wrapper, script).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();

    let output = Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["context-include"])
        .pi(&wrapper)
        .turns([Turn::text("done")])
        .prompt("hello")
        .run()
        .unwrap();
    let prompt = output.system_prompt(0);
    let start = prompt
        .find("<project_instructions path=\"")
        .and_then(|_| prompt.find("notes.md\">"))
        .unwrap_or_else(|| panic!("no include block\n{prompt}\nstderr:\n{}", output.stderr));
    assert!(prompt[start..].contains(MARKER), "{prompt}");
}
