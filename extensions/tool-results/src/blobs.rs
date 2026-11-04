//! Elides long base64 runs, data URIs, and hex dumps, keeping their size.

use std::sync::OnceLock;

use regex::{Captures, Regex};

use crate::lines::group;

fn looks_base64(run: &str) -> bool {
    let body = run.trim_end_matches('=');
    let upper = body.bytes().any(|b| b.is_ascii_uppercase());
    let lower = body.bytes().any(|b| b.is_ascii_lowercase());
    let digit = body.bytes().any(|b| b.is_ascii_digit());
    let slashes = body.bytes().filter(|b| *b == b'/').count();
    upper && lower && digit && slashes * 10 < body.len()
}

fn looks_hex(run: &str) -> bool {
    let digit = run.bytes().any(|b| b.is_ascii_digit());
    let letter = run.bytes().any(|b| b.is_ascii_alphabetic());
    digit && letter && run.bytes().all(|b| b.is_ascii_hexdigit())
}

fn patterns() -> &'static (Regex, Regex) {
    static PATTERNS: OnceLock<(Regex, Regex)> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let data = r"data:([A-Za-z0-9.+-]+/[A-Za-z0-9.+-]+)?((?:;[A-Za-z0-9=.+-]+)*);base64,([A-Za-z0-9+/=]+)";
        let run = r"[A-Za-z0-9+/_-]{16,}={0,2}";
        (Regex::new(data).expect("valid regex"), Regex::new(run).expect("valid regex"))
    })
}

/// Replaces data URI payloads and base64 or hex runs of `min` characters or more.
pub fn elide(text: &str, min: usize) -> Option<String> {
    if text.len() < min {
        return None;
    }
    let (data, run) = patterns();
    let mut changed = false;
    let step = data.replace_all(text, |caps: &Captures| {
        let payload = &caps[3];
        if payload.len() < min {
            return caps[0].to_owned();
        }
        changed = true;
        let mime = caps.get(1).map_or("", |m| m.as_str());
        let params = caps.get(2).map_or("", |m| m.as_str());
        format!(
            "data:{mime}{params};base64,[elided {} characters]",
            group(payload.len())
        )
    });
    let out = run.replace_all(&step, |caps: &Captures| {
        let found = &caps[0];
        let size = group(found.len());
        if found.len() < min {
            found.to_owned()
        } else if looks_hex(found) {
            changed = true;
            format!("[hex elided: {size} characters]")
        } else if looks_base64(found) {
            changed = true;
            format!("[base64 elided: {size} characters]")
        } else {
            found.to_owned()
        }
    });
    changed.then(|| out.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blob(n: usize) -> String {
        "aZ09+/QxbcDEfgHIjkLMnoPQrsTUvwXY"
            .chars()
            .cycle()
            .take(n)
            .collect()
    }

    #[test]
    fn elides_data_uris() {
        let text = format!("<img src=\"data:image/png;base64,{}\">", blob(400));
        assert_eq!(
            elide(&text, 64).unwrap(),
            "<img src=\"data:image/png;base64,[elided 400 characters]\">"
        );
        let small = format!("data:image/png;base64,{}", blob(16));
        assert_eq!(elide(&small, 64), None);
    }

    #[test]
    fn elides_base64_and_hex_runs() {
        let text = format!("key: {}\nsha: {}", blob(512), "deadbeef01".repeat(32));
        assert_eq!(
            elide(&text, 256).unwrap(),
            "key: [base64 elided: 512 characters]\nsha: [hex elided: 320 characters]"
        );
    }

    #[test]
    fn keeps_words_and_paths() {
        let word = "a".repeat(300);
        assert_eq!(elide(&word, 256), None);
        let path = "/usr/lib/x86_64".repeat(30);
        assert_eq!(elide(&path, 256), None);
    }
}
