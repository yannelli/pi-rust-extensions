//! Stack trace folding: repeated frames (recursion) and very long traces.

/// The first line of a frame: JavaScript, Java, and C# `at ...`, Python `File "...", line N`,
/// and Rust backtrace `N: symbol`.
fn is_head(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.len() == line.len() {
        return false;
    }
    if trimmed.starts_with("at ") || trimmed.starts_with("File \"") && trimmed.contains(", line ") {
        return true;
    }
    let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
    digits > 0 && trimmed[digits..].starts_with(": ")
}

/// A line that belongs to the frame above: Python source lines and Rust `at file:line`.
fn is_tail(frame: &str, line: &str) -> bool {
    let indent = |l: &str| l.len() - l.trim_start().len();
    let head = frame.split('\n').next().unwrap_or(frame);
    let (first, trimmed) = (head.trim_start(), line.trim_start());
    if trimmed.is_empty() {
        return false;
    }
    if first.starts_with(|c: char| c.is_ascii_digit()) {
        return trimmed.starts_with("at ");
    }
    first.starts_with("File \"") && !is_head(line) && indent(line) > indent(head)
}

/// Runs of frames, and the lines between them.
enum Part {
    Text(String),
    Frames(Vec<String>),
}

fn parts(text: &str) -> Vec<Part> {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut parts: Vec<Part> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_head(lines[i]) {
            parts.push(Part::Text(lines[i].to_owned()));
            i += 1;
            continue;
        }
        let mut frame = lines[i].to_owned();
        i += 1;
        while i < lines.len() && is_tail(&frame, lines[i]) {
            frame.push('\n');
            frame.push_str(lines[i]);
            i += 1;
        }
        match parts.last_mut() {
            Some(Part::Frames(frames)) => frames.push(frame),
            _ => parts.push(Part::Frames(vec![frame])),
        }
    }
    parts
}

/// Folds a cycle of 1 to 4 frames seen 3 times or more in a row, then keeps the first and
/// last frames of a trace longer than `max`.
pub fn fold(text: &str, max: usize) -> Option<(String, bool)> {
    let mut lossy = false;
    let mut changed = false;
    let mut out: Vec<String> = Vec::new();
    for part in parts(text) {
        let frames = match part {
            Part::Text(line) => {
                out.push(line);
                continue;
            }
            Part::Frames(frames) => frames,
        };
        let (frames, repeated) = fold_cycles(frames);
        changed |= repeated;
        let real = frames.iter().filter(|f| !f.starts_with("    [")).count();
        if real > max.max(2) {
            let tail = (max / 3).max(1);
            let head = max - tail;
            let hidden = frames.len() - head - tail;
            out.extend(frames[..head].iter().cloned());
            out.push(format!("    [... {hidden} frames folded ...]"));
            out.extend(frames[frames.len() - tail..].iter().cloned());
            lossy = true;
            changed = true;
        } else {
            out.extend(frames);
        }
    }
    changed.then(|| (out.join("\n"), lossy))
}

fn fold_cycles(frames: Vec<String>) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    let mut i = 0;
    let mut changed = false;
    while i < frames.len() {
        let mut best: Option<(usize, usize)> = None;
        for period in 1..=4.min(frames.len() - i) {
            let block = &frames[i..i + period];
            let mut reps = 1;
            while i + (reps + 1) * period <= frames.len()
                && &frames[i + reps * period..i + (reps + 1) * period] == block
            {
                reps += 1;
            }
            if reps >= 3 && best.is_none_or(|(p, r)| reps * period > p * r) {
                best = Some((period, reps));
            }
        }
        match best {
            Some((period, reps)) => {
                out.extend(frames[i..i + period].iter().cloned());
                let noun = if period == 1 { "frame" } else { "frames" };
                out.push(format!(
                    "    [... previous {period} {noun} repeated {} more times ...]",
                    reps - 1
                ));
                i += period * reps;
                changed = true;
            }
            None => {
                out.push(frames[i].clone());
                i += 1;
            }
        }
    }
    (out, changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_recursive_frames() {
        let mut text = String::from("RangeError: Maximum call stack size exceeded\n");
        for _ in 0..50 {
            text.push_str("    at recurse (/app/a.js:3:5)\n");
        }
        text.push_str("    at main (/app/a.js:9:1)");
        let (out, lossy) = fold(&text, 30).unwrap();
        assert_eq!(
            out,
            "RangeError: Maximum call stack size exceeded\n    at recurse (/app/a.js:3:5)\n    \
             [... previous 1 frame repeated 49 more times ...]\n    at main (/app/a.js:9:1)"
        );
        assert!(!lossy);
    }

    #[test]
    fn folds_python_frame_pairs() {
        let mut text = String::from("Traceback (most recent call last):\n");
        for _ in 0..4 {
            text.push_str("  File \"/a.py\", line 2, in f\n    return f()\n");
        }
        text.push_str("RecursionError: maximum recursion depth exceeded");
        let (out, _) = fold(&text, 30).unwrap();
        assert!(out.contains("  File \"/a.py\", line 2, in f\n    return f()\n    [... previous 1 frame repeated 3 more times ...]"), "{out}");
        assert!(out.ends_with("RecursionError: maximum recursion depth exceeded"));
    }

    #[test]
    fn keeps_head_and_tail_of_long_traces() {
        let mut text = String::from("panic\n");
        for n in 0..40 {
            text.push_str(&format!(
                "  {n}: crate::f{n}\n             at src/lib.rs:{n}:1\n"
            ));
        }
        let (out, lossy) = fold(text.trim_end(), 9).unwrap();
        assert!(lossy);
        assert!(out.contains("  5: crate::f5\n"), "{out}");
        assert!(out.contains("[... 31 frames folded ...]"), "{out}");
        assert!(
            out.ends_with("  39: crate::f39\n             at src/lib.rs:39:1"),
            "{out}"
        );
        assert!(!out.contains("crate::f6\n"), "{out}");
    }

    #[test]
    fn leaves_short_traces_and_prose() {
        let text = "Error: boom\n    at a (x.js:1:1)\n    at b (x.js:2:1)";
        assert_eq!(fold(text, 30), None);
        assert_eq!(fold("at the end of the day", 30), None);
    }
}
