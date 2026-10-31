//! The files that recognized Bash file commands read or write, for `Read` and `Edit` rules.

use crate::shell::Word;

/// A file a command names, with whether the command writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct FileArg {
    pub path: String,
    pub write: bool,
}

const READERS: &[&str] = &[
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "nl",
    "wc",
    "sort",
    "uniq",
    "cut",
    "strings",
    "od",
    "xxd",
    "hexdump",
    "base64",
    "md5sum",
    "shasum",
    "sha1sum",
    "sha256sum",
    "sha512sum",
    "diff",
    "cmp",
    "file",
    "stat",
];
const WRITERS: &[&str] = &["tee", "touch", "truncate", "rm", "rmdir", "mv"];
/// Commands whose first operand is a pattern or program unless an option supplies it.
const SCRIPTED: &[&str] = &["grep", "egrep", "fgrep", "rg", "sed", "awk"];

/// Options that take a value in the next word, per command.
fn takes_value(command: &str, option: &str) -> bool {
    let common = matches!(
        option,
        "-n" | "-c" | "-k" | "-t" | "-d" | "-f" | "-b" | "-o"
    );
    match command {
        "head" | "tail" => matches!(option, "-n" | "-c"),
        "sort" => matches!(option, "-k" | "-t" | "-o" | "-S" | "-T"),
        "cut" => matches!(option, "-d" | "-f" | "-c" | "-b"),
        "grep" | "egrep" | "fgrep" | "rg" => {
            matches!(option, "-e" | "-f" | "-m" | "-A" | "-B" | "-C")
        }
        "sed" => matches!(option, "-e" | "-f"),
        "awk" => matches!(option, "-f" | "-v" | "-F"),
        "truncate" => matches!(option, "-s" | "-r"),
        "cp" | "mv" | "ln" => option == "-t",
        _ => common && command == "od",
    }
}

/// The files that `words` (one simple command, wrappers stripped) reads or writes.
pub fn file_args(words: &[Word]) -> Vec<FileArg> {
    let Some((first, rest)) = words.split_first() else {
        return Vec::new();
    };
    let command = first.value.rsplit('/').next().unwrap_or_default();
    let mut operands = Vec::new();
    let mut supplied_program = false;
    let mut in_place = false;
    let mut target_dir = None;
    let mut sort_output = None;
    let mut extra_reads = Vec::new();
    let mut options_done = false;
    let mut words = rest.iter();
    while let Some(word) = words.next() {
        let value = word.value.as_str();
        if options_done || !value.starts_with('-') || value == "-" {
            operands.push(value.to_owned());
            continue;
        }
        if value == "--" {
            options_done = true;
            continue;
        }
        if command == "sed"
            && (value == "-i" || value.starts_with("-i") || value.starts_with("--in-place"))
        {
            in_place = true;
            continue;
        }
        if !takes_value(command, value) {
            continue;
        }
        let next = words.next().map(|w| w.value.clone()).unwrap_or_default();
        match (command, value) {
            (_, "-e") => supplied_program = true,
            ("grep" | "egrep" | "fgrep" | "rg" | "sed" | "awk", "-f") => {
                supplied_program = true;
                extra_reads.push(next);
            }
            ("sort", "-o") => sort_output = Some(next),
            (_, "-t") if matches!(command, "cp" | "mv" | "ln") => target_dir = Some(next),
            _ => {}
        }
    }
    let read = |path: String| FileArg { path, write: false };
    let write = |path: String| FileArg { path, write: true };
    let mut out: Vec<FileArg> = extra_reads.into_iter().map(read).collect();
    match command {
        c if READERS.contains(&c) => out.extend(operands.into_iter().map(read)),
        c if WRITERS.contains(&c) => out.extend(operands.into_iter().map(write)),
        c if SCRIPTED.contains(&c) => {
            let files = operands.into_iter().skip(usize::from(!supplied_program));
            out.extend(files.map(if in_place { write } else { read }));
        }
        "cp" | "ln" => {
            if target_dir.is_none()
                && let Some(dest) = operands.pop()
            {
                out.push(write(dest));
            }
            out.extend(operands.into_iter().map(read));
        }
        _ => {}
    }
    if let Some(dir) = target_dir {
        out.push(write(dir));
    }
    if let Some(path) = sort_output {
        out.push(write(path));
    }
    out.retain(|arg| !arg.path.is_empty() && arg.path != "-" && arg.path != "/dev/null");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::parse;

    fn args(command: &str) -> Vec<(String, bool)> {
        let script = parse(command);
        let words = &script.commands[0];
        file_args(words)
            .into_iter()
            .map(|a| (a.path, a.write))
            .collect()
    }

    fn r(path: &str) -> (String, bool) {
        (path.to_owned(), false)
    }

    fn w(path: &str) -> (String, bool) {
        (path.to_owned(), true)
    }

    #[test]
    fn readers_name_their_files() {
        assert_eq!(args("cat .env a.txt"), [r(".env"), r("a.txt")]);
        assert_eq!(args("head -n 5 secrets/key"), [r("secrets/key")]);
        assert_eq!(
            args("sort -o out.txt -k 2 in.txt"),
            [r("in.txt"), w("out.txt")]
        );
        assert_eq!(args("/usr/bin/tail -f app.log"), [r("app.log")]);
    }

    #[test]
    fn scripted_commands_skip_their_pattern() {
        assert_eq!(
            args("grep -n token .env src/a.rs"),
            [r(".env"), r("src/a.rs")]
        );
        assert_eq!(args("grep -e token .env"), [r(".env")]);
        assert_eq!(args("sed -i 's/a/b/' config.toml"), [w("config.toml")]);
        assert_eq!(args("sed -n 1p .env"), [r(".env")]);
        assert_eq!(
            args("awk -f prog.awk data.csv"),
            [r("prog.awk"), r("data.csv")]
        );
        assert!(args("grep -r token .").contains(&r(".")));
    }

    #[test]
    fn writers_and_copies() {
        assert_eq!(args("tee -a build.log"), [w("build.log")]);
        assert_eq!(args("rm -rf secrets"), [w("secrets")]);
        assert_eq!(args("cp src.txt dst.txt"), [w("dst.txt"), r("src.txt")]);
        assert_eq!(args("cp -t out a b"), [r("a"), r("b"), w("out")]);
        assert_eq!(args("mv a b"), [w("a"), w("b")]);
        assert!(args("echo hi").is_empty());
        assert!(args("cat - /dev/null").is_empty());
    }
}
