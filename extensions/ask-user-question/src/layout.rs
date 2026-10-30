//! Plain-text layout helpers: word wrap, boxes, and two-column joins.

use pi_ext_sdk::style::{char_width, fit, visible_width};

/// Wraps plain text to `width` columns; keeps blank lines and breaks long words.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if visible_width(&candidate) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            line = break_word(word, width, &mut out);
        }
        out.push(line);
    }
    out
}

/// Pushes full-width chunks of `word` into `out` and returns the remainder.
fn break_word(word: &str, width: usize, out: &mut Vec<String>) -> String {
    let mut chunk = String::new();
    let mut used = 0;
    for c in word.chars() {
        let w = char_width(c);
        if used + w > width && !chunk.is_empty() {
            out.push(std::mem::take(&mut chunk));
            used = 0;
        }
        chunk.push(c);
        used += w;
    }
    chunk
}

/// Wraps `text` and prefixes each line; the first line gets `first`, the rest `rest`.
pub fn wrap_prefixed(text: &str, width: usize, first: &str, rest: &str) -> Vec<String> {
    let body = width.saturating_sub(visible_width(first)).max(1);
    wrap(text, body)
        .into_iter()
        .enumerate()
        .map(|(i, line)| format!("{}{line}", if i == 0 { first } else { rest }))
        .collect()
}

/// A bordered box `width` columns wide around `lines`.
pub fn boxed(lines: &[String], width: usize) -> Vec<String> {
    let inner = width.saturating_sub(4).max(1);
    let mut out = vec![format!("┌{}┐", "─".repeat(inner + 2))];
    for line in lines.iter().flat_map(|l| wrap(l, inner)) {
        out.push(format!("│ {} │", fit(&line, inner)));
    }
    out.push(format!("└{}┘", "─".repeat(inner + 2)));
    out
}

/// Places `left` and `right` side by side; `left` is padded to `left_width`.
pub fn columns(left: &[String], right: &[String], left_width: usize, gap: usize) -> Vec<String> {
    let rows = left.len().max(right.len());
    (0..rows)
        .map(|i| {
            let l = left.get(i).map_or("", String::as_str);
            let r = right.get(i).map_or("", String::as_str);
            format!("{}{}{r}", fit(l, left_width), " ".repeat(gap))
                .trim_end()
                .to_owned()
        })
        .collect()
}
