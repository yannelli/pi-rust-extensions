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

/// The bin dirs and env files of the enabled plugins whose executables the command names.
pub fn dirs_for(env: &Env, command: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (mut dirs, mut files) = (Vec::new(), Vec::new());
    for scope in env.visible_scopes() {
        let Ok(state) = State::load(&env.dirs(scope)) else {
            continue;
        };
        for record in state.plugins.values().filter(|r| r.enabled) {
            let mut named = false;
            for bin in &record.bins {
                let hit = words(command).any(|w| bin.names.iter().any(|n| n == w));
                if hit && bin.dir.is_dir() && !dirs.contains(&bin.dir) {
                    dirs.push(bin.dir.clone());
                }
                named |= hit;
            }
            let file = record.env_file.as_ref().filter(|f| named && f.is_file());
            files.extend(file.cloned());
        }
    }
    (dirs, files)
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The plugin env as a shell file: one `export NAME='value'` line each.
pub fn env_script(vars: &[(String, String)]) -> String {
    let mut out = String::new();
    for (key, value) in vars {
        let valid = key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !key.starts_with(|c: char| c.is_ascii_digit());
        if valid {
            out.push_str(&format!("export {key}={}\n", quote(value)));
        }
    }
    out
}

/// The bash command with the plugin env files sourced and the bin dirs appended to `PATH`,
/// or `None` when it names no bin file. The env stays in the 0600 file, not in the command.
pub fn rewrite(env: &Env, command: &str) -> Option<String> {
    if cfg!(windows) {
        return None;
    }
    let (dirs, files) = dirs_for(env, command);
    if dirs.is_empty() {
        return None;
    }
    let sources: String = files
        .iter()
        .map(|f| format!(". {}; ", quote(&f.display().to_string())))
        .collect();
    let path: String = dirs
        .iter()
        .map(|d| format!(":{}", quote(&d.display().to_string())))
        .collect();
    Some(format!("{sources}export PATH=\"$PATH\"{path}; {command}"))
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
        let vars = vec![
            ("A_1".to_owned(), "x y".to_owned()),
            ("1B".to_owned(), "z".to_owned()),
        ];
        assert_eq!(env_script(&vars), "export A_1='x y'\n");
    }
}
