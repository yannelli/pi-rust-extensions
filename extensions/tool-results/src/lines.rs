//! Line transforms: whitespace, repeated lines, and long-line clipping.

/// Drops trailing whitespace, keeps at most one blank line in a row, and trims blank lines
/// at both ends.
pub fn whitespace(text: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim_end();
        if line.is_empty() && out.last().is_none_or(|l| l.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let out = out.join("\n");
    (out != text).then_some(out)
}

/// Collapses runs of spaces and tabs inside each line to one space; indentation stays.
pub fn collapse_spaces(text: &str) -> Option<String> {
    let mut out = String::with_capacity(text.len());
    for (n, line) in text.split('\n').enumerate() {
        if n > 0 {
            out.push('\n');
        }
        let body = line.trim_start_matches([' ', '\t']);
        out.push_str(&line[..line.len() - body.len()]);
        let mut gap = false;
        for c in body.chars() {
            if c == ' ' || c == '\t' {
                gap = true;
                continue;
            }
            if gap {
                out.push(' ');
                gap = false;
            }
            out.push(c);
        }
        if gap {
            out.push(' ');
        }
    }
    (out != text).then_some(out)
}

/// Collapses a line, or a block of up to 4 lines, that repeats `min` times or more in a row
/// into one copy and a count line, when that is shorter.
pub fn dedupe(text: &str, min: usize) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    let mut changed = false;
    while i < lines.len() {
        let mut best: Option<(usize, usize)> = None;
        for period in 1..=4 {
            if i + period > lines.len() || lines[i..i + period].iter().all(|l| l.trim().is_empty())
            {
                continue;
            }
            let block = &lines[i..i + period];
            let mut reps = 1;
            while i + (reps + 1) * period <= lines.len()
                && &lines[i + reps * period..i + (reps + 1) * period] == block
            {
                reps += 1;
            }
            if reps >= min.max(2) && best.is_none_or(|(_, r)| reps * period > r) {
                best = Some((period, reps * period));
            }
        }
        let marker = |period: usize, covered: usize| {
            let more = covered / period - 1;
            let text = match period {
                1 => format!("[previous line repeated {more} more times]"),
                _ => format!("[previous {period} lines repeated {more} more times]"),
            };
            let removed: usize = lines[i + period..i + covered]
                .iter()
                .map(|l| l.len() + 1)
                .sum();
            (removed > text.len() + 1).then_some((period, covered, text))
        };
        match best.and_then(|(p, c)| marker(p, c)) {
            Some((period, covered, text)) => {
                out.extend(lines[i..i + period].iter().map(|l| (*l).to_owned()));
                out.push(text);
                i += covered;
                changed = true;
            }
            None => {
                out.push(lines[i].to_owned());
                i += 1;
            }
        }
    }
    changed.then(|| out.join("\n"))
}

/// Clips lines over `max` characters, such as minified code, and says how much was cut.
pub fn long_lines(text: &str, max: usize) -> Option<String> {
    if text.len() <= max {
        return None;
    }
    let mut changed = false;
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let count = line.chars().count();
            if count <= max {
                return line.to_owned();
            }
            changed = true;
            let kept: String = line.chars().take(max).collect();
            format!(
                "{kept} [... {} more characters on this line]",
                group(count - max)
            )
        })
        .collect();
    changed.then(|| lines.join("\n"))
}

/// `12345` as `12,345`.
pub fn group(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// `121345` as `121k`, `20500` as `20.5k`, and `1234567` as `1.2M`.
pub fn short(n: usize) -> String {
    match n {
        0..1000 => n.to_string(),
        1000..100_000 => trim_zero(format!("{:.1}k", n as f64 / 1000.0)),
        100_000..1_000_000 => format!("{}k", n / 1000),
        _ => trim_zero(format!("{:.1}M", n as f64 / 1_000_000.0)),
    }
}

fn trim_zero(text: String) -> String {
    text.replace(".0k", "k").replace(".0M", "M")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whitespace_trims_and_collapses_blank_runs() {
        let text = "\n\na  \nb\t\n\n\n\nc\n\n";
        assert_eq!(whitespace(text).unwrap(), "a\nb\n\nc");
        assert_eq!(whitespace("a\n\nb"), None);
    }

    #[test]
    fn collapse_spaces_keeps_indentation() {
        assert_eq!(collapse_spaces("  a    b\t\tc").unwrap(), "  a b c");
        assert_eq!(collapse_spaces("a b"), None);
    }

    #[test]
    fn dedupe_lines_and_blocks() {
        let text = "start\nwarn: x is unused\nwarn: x is unused\nwarn: x is unused\nwarn: x is unused\nend";
        assert_eq!(
            dedupe(text, 3).unwrap(),
            "start\nwarn: x is unused\n[previous line repeated 3 more times]\nend"
        );
        assert_eq!(dedupe("a\na\nb", 3), None);
        assert_eq!(dedupe("ab\nab\nab\nab", 3), None);
        let block = "GET /api/users\n200 OK\n".repeat(3) + "done";
        assert_eq!(
            dedupe(&block, 3).unwrap(),
            "GET /api/users\n200 OK\n[previous 2 lines repeated 2 more times]\ndone"
        );
        assert_eq!(dedupe("\n\n\n\n", 2), None);
    }

    #[test]
    fn clips_long_lines() {
        let text = format!("short\n{}", "x".repeat(300));
        let out = long_lines(&text, 100).unwrap();
        assert_eq!(
            out,
            format!(
                "short\n{} [... 200 more characters on this line]",
                "x".repeat(100)
            )
        );
        assert_eq!(long_lines("tiny", 100), None);
    }

    #[test]
    fn number_formats() {
        assert_eq!(group(1234567), "1,234,567");
        assert_eq!(group(12), "12");
        assert_eq!(short(121_345), "121k");
        assert_eq!(short(20_500), "20.5k");
        assert_eq!(short(20_000), "20k");
        assert_eq!(short(1_250_000), "1.2M");
        assert_eq!(short(999), "999");
    }
}
