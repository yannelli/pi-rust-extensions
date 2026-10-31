//! rewind under the pi binary logs a warning only when a checkpoint really fails.

use pi_ext_e2e::{Output, Run, Turn, Value, cargo_bin, json, pi_bin_or_skip};

const FAKE_GIT: [(&str, &str); 4] = [
    ("work/.git/HEAD", "ref: refs/heads/main\n"),
    (
        "work/.git/config",
        "[core]\n\trepositoryformatversion = 0\n\tbare = false\n",
    ),
    ("work/.git/objects/info/packs", ""),
    ("work/.git/refs/tags/.keep", ""),
];

fn rewind() -> Run {
    Run::new(cargo_bin("pi-rust-ext", "pi-rust-ext"))
        .args(["rewind"])
        .env("PI_EXT_LOG", "info")
}

fn run(run: Run, turns: Vec<Turn>) -> Output {
    let output = run.turns(turns).prompt("Do the task").run().unwrap();
    assert!(output.status.success(), "{}", output.stderr);
    output
}

fn warnings(output: &Output) -> Vec<&str> {
    let lines = output.stderr.lines();
    lines
        .filter(|line| line.contains("warn:") || line.contains("error:"))
        .collect()
}

/// `files=<n>` of the one checkpoint log line.
fn checkpoint_files(output: &Output) -> usize {
    let lines: Vec<&str> = output
        .stderr
        .lines()
        .filter(|line| line.contains("info: checkpoint ") && line.contains(" files="))
        .collect();
    assert_eq!(lines.len(), 1, "{}", output.stderr);
    let files = lines[0].rsplit("files=").next().unwrap_or_default();
    files.trim().parse().unwrap()
}

fn assert_silent(output: &Output) {
    assert!(warnings(output).is_empty(), "{}", output.stderr);
}

#[test]
fn no_op_turn_is_silent_outside_git() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let output = run(rewind(), vec![Turn::text("Nothing to change.")]);
    assert_silent(&output);
    assert_eq!(checkpoint_files(&output), 0);
}

#[test]
fn no_op_turn_is_silent_in_a_git_repo_with_untracked_files() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let mut setup = rewind()
        .file("work/.gitignore", "ignored.txt\n")
        .file("work/ignored.txt", "secret\n")
        .file("work/untracked.txt", "draft\n");
    for (path, text) in FAKE_GIT {
        setup = setup.file(path, text);
    }
    let output = run(setup, vec![Turn::text("Nothing to change.")]);
    assert_silent(&output);
    assert_eq!(checkpoint_files(&output), 0);
}

#[test]
fn large_file_is_skipped_silently() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let setup = rewind()
        .settings(json!({"ayu": {"checkpoint": {"maxFileMB": 1}}}))
        .file("work/big.bin", "x".repeat(3 * 1024 * 1024));
    let write = json!({"path": "note.txt", "content": "hello\n"});
    let output = run(
        setup,
        vec![Turn::tool_call("write", write), Turn::text("Wrote it.")],
    );
    assert_silent(&output);
    assert_eq!(checkpoint_files(&output), 1);
}

#[test]
fn unreadable_file_is_skipped_silently() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let setup = rewind()
        .file("work/locked.txt", "one\n")
        .file("work/open.txt", "one\n");
    let command = "chmod 000 locked.txt && echo two > open.txt";
    let output = run(
        setup,
        vec![
            Turn::tool_call("bash", json!({"command": command})),
            Turn::text("Done."),
        ],
    );
    assert_silent(&output);
    assert!(
        output
            .stderr
            .contains("skipped unreadable files: locked.txt"),
        "{}",
        output.stderr
    );
    assert_eq!(checkpoint_files(&output), 1);
}

#[test]
fn two_sessions_at_once_are_silent() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let sessions: Vec<_> = (0..2)
        .map(|i| {
            std::thread::spawn(move || {
                let write = json!({"path": format!("s{i}.txt"), "content": "x\n"});
                run(
                    rewind(),
                    vec![Turn::tool_call("write", write), Turn::text("Done.")],
                )
            })
        })
        .collect();
    for session in sessions {
        let output = session.join().unwrap();
        assert_silent(&output);
        assert_eq!(checkpoint_files(&output), 1);
    }
}

#[test]
fn a_failed_checkpoint_warns_once_per_prompt() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let setup = rewind().file("agent/ext/rewind/sessions", "not a directory");
    let bash: Value = json!({"command": "echo one"});
    let output = run(
        setup,
        vec![
            Turn::tool_call("bash", bash.clone()),
            Turn::tool_call("bash", bash),
            Turn::text("Done."),
        ],
    );
    let failed: Vec<&str> = warnings(&output)
        .into_iter()
        .filter(|line| line.contains("checkpoint before turn failed"))
        .collect();
    assert_eq!(failed.len(), 1, "{}", output.stderr);
}

#[test]
fn checkpoints_work_without_a_session_file() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let write = json!({"path": "note.txt", "content": "hello\n"});
    let output = run(
        rewind().pi_args(["--no-session"]),
        vec![Turn::tool_call("write", write), Turn::text("Wrote it.")],
    );
    assert_silent(&output);
    assert_eq!(checkpoint_files(&output), 1);
}

#[test]
fn a_large_first_snapshot_runs_in_budgeted_steps_without_warnings() {
    if pi_bin_or_skip().is_none() {
        return;
    }
    let budget = json!({"ayu": {"checkpoint": {"stageBudgetSeconds": 0.001}}});
    let mut setup = rewind().settings(budget);
    for i in 0..1200 {
        setup = setup.file(format!("work/d{}/f{i}.txt", i / 100), format!("{i}\n"));
    }
    let write = json!({"path": "note.txt", "content": "hello\n"});
    let output = run(
        setup,
        vec![Turn::tool_call("write", write), Turn::text("Wrote it.")],
    );
    assert_silent(&output);
    assert!(
        output
            .stderr
            .contains("snapshot staged 500 files, 700 left"),
        "{}",
        output.stderr
    );
    assert!(
        !output.stderr.contains("info: checkpoint "),
        "{}",
        output.stderr
    );
}
