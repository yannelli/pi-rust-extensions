//! Claude command argument placeholders as pi prompt template placeholders (`$1` is the first
//! argument in pi).

use crate::settings::ArgIndex;

/// `$<digits>` at `after` (the text after a `$`): the number and its length.
fn number(after: &str) -> Option<(usize, usize)> {
    let len = after
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(after.len());
    if len == 0 {
        return None;
    }
    Some((after[..len].parse().ok()?, len))
}

/// `true` when `$N` counts from 0. Auto: the current Claude docs count from 0, and a file
/// that uses `$0`, `$ARGUMENTS[N]`, or named `arguments` follows them; older files count from 1.
pub fn zero_based(body: &str, names: &[String], index: ArgIndex) -> bool {
    match index {
        ArgIndex::Zero => true,
        ArgIndex::One => false,
        ArgIndex::Auto => {
            !names.is_empty()
                || body.contains("$ARGUMENTS[")
                || body
                    .match_indices('$')
                    .any(|(i, _)| number(&body[i + 1..]).is_some_and(|(n, _)| n == 0))
        }
    }
}

/// Maps `$ARGUMENTS[N]` (0-based), named `$<arg>`, and `$N` (0-based when `zero`) to pi's
/// 1-based `$N`. `$ARGUMENTS`, `$@`, and `${...}` stay.
pub fn map(body: &str, names: &[String], zero: bool) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let word_end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        let word = &after[..word_end];
        let indexed = (word == "ARGUMENTS")
            .then(|| after[word_end..].strip_prefix('['))
            .flatten()
            .and_then(|inner| {
                let close = inner.find(']')?;
                let n: usize = inner[..close].trim().parse().ok()?;
                Some((n, word_end + 2 + close))
            });
        if let Some((n, used)) = indexed {
            out.push_str(&format!("${}", n + 1));
            rest = &after[used..];
        } else if let Some((n, len)) = number(after) {
            out.push_str(&format!("${}", if zero { n + 1 } else { n }));
            rest = &after[len..];
        } else if let Some(index) = names.iter().position(|name| name == word) {
            out.push_str(&format!("${}", index + 1));
            rest = &after[word_end..];
        } else {
            out.push('$');
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

/// `true` when the body uses any pi argument placeholder.
pub fn has_placeholder(body: &str) -> bool {
    body.match_indices('$').any(|(i, _)| {
        let after = &body[i + 1..];
        after.starts_with("ARGUMENTS")
            || after.starts_with('@')
            || after.starts_with("{@")
            || after.starts_with("{ARGUMENTS")
            || after.starts_with(|c: char| c.is_ascii_digit())
            || after
                .strip_prefix('{')
                .is_some_and(|a| a.starts_with(|c: char| c.is_ascii_digit()))
    })
}

/// Maps the placeholders. Like Claude Code, a body without placeholders gets the arguments
/// appended; pi templates cannot add a label only when there are arguments, so there is none.
pub fn convert(body: &str, names: &[String], index: ArgIndex) -> String {
    let mut body = map(body, names, zero_based(body, names, index));
    if !has_placeholder(&body) {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("\n$ARGUMENTS\n");
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        vec!["file".to_owned(), "mode".to_owned()]
    }

    #[test]
    fn indexed_and_named() {
        let body = "A $ARGUMENTS[0] B $ARGUMENTS[1] $file/$mode $files $ARGUMENTS cost $";
        assert_eq!(
            convert(body, &names(), ArgIndex::Auto),
            "A $1 B $2 $1/$2 $files $ARGUMENTS cost $"
        );
    }

    #[test]
    fn auto_detects_the_indexing() {
        assert_eq!(
            convert("Use $0 then $1", &[], ArgIndex::Auto),
            "Use $1 then $2"
        );
        assert_eq!(
            convert("Use $1 then $2", &[], ArgIndex::Auto),
            "Use $1 then $2"
        );
        assert_eq!(convert("$ARGUMENTS[0] $1", &[], ArgIndex::Auto), "$1 $2");
        assert_eq!(convert("$file $1", &names(), ArgIndex::Auto), "$1 $2");
        assert_eq!(convert("Use $10", &[], ArgIndex::Auto), "Use $10");
    }

    #[test]
    fn the_switch_wins() {
        assert_eq!(convert("Use $1", &[], ArgIndex::Zero), "Use $2");
        assert_eq!(convert("Use $0 $1", &[], ArgIndex::One), "Use $0 $1");
    }

    #[test]
    fn appends_arguments_without_a_label() {
        assert_eq!(
            convert("Do it", &[], ArgIndex::Auto),
            "Do it\n\n$ARGUMENTS\n"
        );
        assert_eq!(
            convert("Do it\n", &[], ArgIndex::Auto),
            "Do it\n\n$ARGUMENTS\n"
        );
        assert_eq!(
            convert("All: $ARGUMENTS", &[], ArgIndex::Auto),
            "All: $ARGUMENTS"
        );
        assert_eq!(convert("Rest ${@:2}", &[], ArgIndex::Auto), "Rest ${@:2}");
    }
}
