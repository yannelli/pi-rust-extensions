//! The diff of an `edit` result (`details.diff`: `+12 text`, `-12 text`, ` 12 text`, and `...`
//! gaps) with line numbers and tinted rows.

use pi_ext_sdk::style::{truncate, visible_width};
use serde_json::Value;

use crate::paint::Paint;

enum Line<'a> {
    Added(&'a str, &'a str),
    Removed(&'a str, &'a str),
    Context(&'a str, &'a str),
    Gap,
}

fn parse(raw: &str) -> Line<'_> {
    let (sign, rest) = raw.split_at(raw.chars().next().map_or(0, char::len_utf8));
    let rest = rest.trim_start();
    if rest.trim() == "..." || rest.is_empty() && sign == " " {
        return Line::Gap;
    }
    let (number, text) = rest.split_once(' ').unwrap_or((rest, ""));
    match sign {
        "+" => Line::Added(number, text),
        "-" => Line::Removed(number, text),
        _ => Line::Context(number, text),
    }
}

/// A summary line and up to 14 diff lines (400 when expanded), each `width` columns wide.
pub fn render(
    paint: &Paint,
    diff: &str,
    args: &Value,
    width: usize,
    expanded: bool,
    more: impl Fn(usize) -> String,
) -> Vec<String> {
    let lines: Vec<Line> = diff.lines().map(parse).collect();
    let added = lines
        .iter()
        .filter(|line| matches!(line, Line::Added(..)))
        .count();
    let removed = lines
        .iter()
        .filter(|line| matches!(line, Line::Removed(..)))
        .count();
    let edits = args
        .get("edits")
        .and_then(Value::as_array)
        .map_or(1, Vec::len);
    let blocks = if edits > 1 {
        format!(" in {edits} places")
    } else {
        String::new()
    };
    let summary = format!("{added} added, {removed} removed{blocks}");
    let number_width = lines
        .iter()
        .filter_map(|line| match line {
            Line::Added(number, _) | Line::Removed(number, _) | Line::Context(number, _) => {
                Some(number.len())
            }
            Line::Gap => None,
        })
        .max()
        .unwrap_or(1);
    let limit = if expanded { 400 } else { 14 };
    let mut out = vec![paint.fg("muted", &summary)];
    for line in lines.iter().take(limit) {
        out.push(row(paint, line, number_width, width));
    }
    if lines.len() > limit {
        out.push(more(lines.len() - limit));
    }
    out
}

fn row(paint: &Paint, line: &Line, number_width: usize, width: usize) -> String {
    let number = |number: &str| paint.fg("dim", &format!("{number:>number_width$}"));
    let (text, tint) = match line {
        Line::Gap => return paint.fg("dim", &format!("{:>number_width$}   ⋮", "")),
        Line::Added(at, text) => (
            format!(
                "{} {} {}",
                number(at),
                paint.fg("toolDiffAdded", "+"),
                paint.fg("toolDiffAdded", text)
            ),
            "toolSuccessBg",
        ),
        Line::Removed(at, text) => (
            format!(
                "{} {} {}",
                number(at),
                paint.fg("toolDiffRemoved", "-"),
                paint.fg("toolDiffRemoved", text)
            ),
            "toolErrorBg",
        ),
        Line::Context(at, text) => {
            return format!("{}   {}", number(at), paint.fg("toolDiffContext", text));
        }
    };
    let text = truncate(&text.replace('\t', "  "), width);
    if !paint.has_bg(tint) {
        return text;
    }
    let pad = " ".repeat(width.saturating_sub(visible_width(&text)));
    paint.bg(tint, &format!("{text}{pad}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pi_ext_sdk::style::strip;
    use serde_json::json;

    #[test]
    fn numbers_signs_and_gaps() {
        let diff = " 1 fn main() {\n-2     old();\n+2     new();\n   ...\n 9 }";
        let lines = render(
            &Paint::default(),
            diff,
            &json!({ "edits": [{}, {}] }),
            30,
            false,
            |n| format!("+{n}"),
        );
        let plain: Vec<String> = lines.iter().map(|line| strip(line)).collect();
        assert_eq!(
            plain,
            [
                "1 added, 1 removed in 2 places",
                "1   fn main() {",
                "2 -     old();",
                "2 +     new();",
                "    ⋮",
                "9   }"
            ]
        );
    }
}
