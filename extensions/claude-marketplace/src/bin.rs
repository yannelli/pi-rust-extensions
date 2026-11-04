//! Claude plugin `bin/` executables on the `PATH` of the bash tool. Like Claude Code, the
//! plugin dirs come after the user's `PATH`, so they cannot shadow system commands.

use std::path::{Path, PathBuf};

use crate::env::Env;
use crate::state::State;

/// The names of the regular files in a plugin's `bin/`.
pub fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort();
    names
}

/// The words of a shell command, split at characters that cannot be in a command name.
fn words(command: &str) -> impl Iterator<Item = &str> {
    command
        .split(|c: char| !(c.is_ascii_alphanumeric() || "._+-".contains(c)))
        .filter(|w| !w.is_empty())
}

/// The bin dirs of the enabled plugins whose executables the command names.
pub fn dirs_for(env: &Env, command: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for scope in env.visible_scopes() {
        let Ok(state) = State::load(&env.dirs(scope)) else {
            continue;
        };
        for record in state.plugins.values().filter(|r| r.enabled) {
            let Some(dir) = &record.bin else { continue };
            let named = words(command).any(|w| record.bin_names.iter().any(|n| n == w));
            if named && dir.is_dir() && !out.contains(dir) {
                out.push(dir.clone());
            }
        }
    }
    out
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The bash command with the bin dirs appended to `PATH`, or `None` when it names none.
pub fn rewrite(env: &Env, command: &str) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let dirs = dirs_for(env, command);
    if dirs.is_empty() {
        return None;
    }
    let path: String = dirs
        .iter()
        .map(|d| format!(":{}", quote(&d.display().to_string())))
        .collect();
    Some(format!("export PATH=\"$PATH\"{path}; {command}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_command_words() {
        let found: Vec<&str> = words("cd x && ./bin/fmt-tool --a|grep y").collect();
        assert_eq!(
            found,
            ["cd", "x", ".", "bin", "fmt-tool", "--a", "grep", "y"]
        );
        assert_eq!(quote("/a'b"), r"'/a'\''b'");
    }
}
