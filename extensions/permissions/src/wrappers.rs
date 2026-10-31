use crate::shell::Word;

const WRAPPERS: &[&str] = &[
    "timeout", "time", "nice", "nohup", "stdbuf", "builtin", "noglob",
];

pub fn is_assignment(word: &str) -> bool {
    let Some((name, _)) = word.split_once('=') else {
        return false;
    };
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Drops wrappers that run their arguments (`timeout 30`, `nohup`, bare `xargs`, ...).
/// Leading `NAME=value` words are dropped only when `assignments` is true.
pub fn strip_wrappers(words: &[Word], assignments: bool) -> &[Word] {
    let mut rest = words;
    while let Some(first) = rest.first() {
        let name = first.value.as_str();
        let next_is_word = rest.get(1).is_some_and(|w| !w.value.starts_with('-'));
        if assignments && is_assignment(&first.raw) {
            rest = &rest[1..];
        } else if WRAPPERS.contains(&name) {
            rest = &rest[1..];
            while let Some(flag) = rest.first().filter(|w| w.value.starts_with('-')) {
                let takes_arg = matches!(
                    (name, flag.value.as_str()),
                    ("nice", "-n") | ("timeout", "-s" | "-k" | "--signal" | "--kill-after")
                );
                rest = &rest[(if takes_arg { 2 } else { 1 }).min(rest.len())..];
            }
            if name == "timeout" && !rest.is_empty() {
                rest = &rest[1..];
            }
        } else if matches!(name, "command" | "xargs") && next_is_word {
            rest = &rest[1..];
        } else {
            break;
        }
    }
    rest
}

pub fn raw_text(words: &[Word]) -> String {
    let raws: Vec<&str> = words.iter().map(|w| w.raw.as_str()).collect();
    raws.join(" ")
}

pub fn values(words: &[Word]) -> Vec<String> {
    words.iter().map(|w| w.value.clone()).collect()
}
