//! Terminal garbage: ANSI escapes, carriage-return progress bars, spinners, and control bytes.

/// Removes CSI, OSC, DCS/APC/PM/SOS strings, two-byte escapes, and 8-bit CSI sequences.
pub fn strip_ansi(text: &str) -> Option<String> {
    if !text.contains(['\x1b', '\u{9b}']) {
        return None;
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\u{9b}' {
            i = skip_csi(&chars, i + 1);
            continue;
        }
        if c != '\x1b' {
            out.push(c);
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('[') => i = skip_csi(&chars, i + 2),
            Some(']' | 'P' | 'X' | '^' | '_') => i = skip_string(&chars, i + 2),
            Some(c) if ('\x20'..='\x2f').contains(c) => {
                i += 2;
                while i < chars.len() && ('\x20'..='\x2f').contains(&chars[i]) {
                    i += 1;
                }
                i += 1;
            }
            Some(_) => i += 2,
            None => i += 1,
        }
    }
    Some(out)
}

fn skip_csi(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() && ('\x20'..='\x3f').contains(&chars[i]) {
        i += 1;
    }
    (i + 1).min(chars.len())
}

/// Skips to the end of an OSC-like string: BEL or `ESC \`.
fn skip_string(chars: &[char], mut i: usize) -> usize {
    while i < chars.len() {
        match chars[i] {
            '\x07' => return i + 1,
            '\x1b' if chars.get(i + 1) == Some(&'\\') => return i + 2,
            _ => i += 1,
        }
    }
    i
}

fn is_spinner(c: char) -> bool {
    ('\u{2801}'..='\u{28ff}').contains(&c) || "◐◓◑◒◴◷◶◵⣾⣽⣻⢿⡿⣟⣯⣷".contains(c)
}

fn is_bar(c: char) -> bool {
    "█▉▊▋▌▍▎▏▓▒░■□━─".contains(c)
}

/// A line that reports progress: a percentage, a bar, a spinner, or an ETA.
fn is_progress(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.chars().next().is_some_and(is_spinner) {
        return true;
    }
    let bytes = line.as_bytes();
    let percent = bytes
        .windows(2)
        .any(|w| w[0].is_ascii_digit() && w[1] == b'%');
    let glyph_bar = line.chars().filter(|c| is_bar(*c)).count() >= 3;
    let ascii_bar = line.contains("[=") || line.contains("==>") || line.contains("##########");
    percent || glyph_bar || ascii_bar || line.to_ascii_lowercase().contains(" eta ")
}

/// The line with digits, spinner frames, and bar glyphs folded, to spot updates of one bar.
fn shape(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars() {
        let mapped = match c {
            '0'..='9' => '0',
            c if is_spinner(c) => '*',
            c if is_bar(c) || "=#>-. ".contains(c) => '#',
            c => c,
        };
        if !(out.ends_with(mapped) && (mapped == '0' || mapped == '#')) {
            out.push(mapped);
        }
    }
    out
}

/// Two updates of one bar: the same shape up to the first `%`, or in full without one.
fn same_bar(a: &str, b: &str) -> bool {
    let head = |line: &str| {
        let shape = shape(line);
        match shape.find('%') {
            Some(at) => shape[..=at].to_owned(),
            None => shape,
        }
    };
    head(a) == head(b)
}

/// Applies `\r` and backspace as a terminal does, drops spinner-only lines, and keeps the
/// last line of each run of updates of one progress bar.
pub fn progress(text: &str) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in text.replace("\r\n", "\n").split('\n') {
        let line = raw
            .rsplit('\r')
            .find(|part| !part.trim().is_empty())
            .unwrap_or("");
        let line = if line.contains('\x08') {
            backspace(line)
        } else {
            line.to_owned()
        };
        let trimmed = line.trim();
        if !trimmed.is_empty() && trimmed.chars().all(is_spinner) {
            continue;
        }
        if let Some(last) = lines.last_mut() {
            if is_progress(&line) && is_progress(last) && same_bar(last, &line) {
                *last = line;
                continue;
            }
        }
        lines.push(line);
    }
    let out = lines.join("\n");
    (out != text).then_some(out)
}

fn backspace(line: &str) -> String {
    let mut out = String::new();
    for c in line.chars() {
        if c == '\x08' {
            out.pop();
        } else {
            out.push(c);
        }
    }
    out
}

/// Drops NUL and the other C0 and C1 controls; keeps `\n` and `\t`, and `\r` and ESC when
/// `keep_cr` or `keep_esc` say so (their own features are off).
pub fn control(text: &str, keep_cr: bool, keep_esc: bool) -> Option<String> {
    let drop = |c: char| match c {
        '\n' | '\t' => false,
        '\r' => !keep_cr,
        '\x1b' => !keep_esc,
        c => c.is_control(),
    };
    if !text.chars().any(drop) {
        return None;
    }
    Some(text.chars().filter(|c| !drop(*c)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_ansi_sequences() {
        let text = "\x1b[1;31merror\x1b[0m: \x1b]8;;http://x\x07link\x1b]8;;\x07 \x1b(Bok\x1b[K";
        assert_eq!(strip_ansi(text).unwrap(), "error: link ok");
        assert_eq!(strip_ansi("plain"), None);
        assert_eq!(strip_ansi("\u{9b}2Jclear").unwrap(), "clear");
        assert_eq!(strip_ansi("\x1bP1$r0m\x1b\\done").unwrap(), "done");
    }

    #[test]
    fn resolves_carriage_return_progress() {
        let text = "Downloading  10%\rDownloading  50%\rDownloading 100%\ndone";
        assert_eq!(progress(text).unwrap(), "Downloading 100%\ndone");
        assert_eq!(progress("a\r\nb"), Some("a\nb".to_owned()));
        assert_eq!(progress("ab\x08c"), Some("ac".to_owned()));
        assert_eq!(progress("no progress here\nok"), None);
    }

    #[test]
    fn collapses_progress_updates_and_spinners() {
        let text = "start\n[=>    ] 10% eta 5s\n[===>  ] 60% eta 2s\n[======] 100%\nnext";
        assert_eq!(progress(text).unwrap(), "start\n[======] 100%\nnext");
        let spin = "⠋\n⠙\n⠹ Compiling foo\n⠸ Compiling foo\nfinished";
        assert_eq!(progress(spin).unwrap(), "⠸ Compiling foo\nfinished");
        let bars = "███░░░ 3/6\n██████ 6/6";
        assert_eq!(progress(bars).unwrap(), "██████ 6/6");
        let distinct = "10% of tests\nall 100% green";
        assert_eq!(progress(distinct), None);
    }

    #[test]
    fn drops_control_characters() {
        assert_eq!(
            control("a\0b\x07c\td\ne", false, false).unwrap(),
            "abc\td\ne"
        );
        assert_eq!(control("a\rb\x1b", true, true), None);
        assert_eq!(control("a\u{85}b", false, false).unwrap(), "ab");
        assert_eq!(control("clean", false, false), None);
    }
}
