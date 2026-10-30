use crate::scan::Diagnostic;

/// JavaScript `String(n)` for finite non-negative numbers.
pub fn js_number(n: f64) -> String {
    if n.fract() == 0.0 && n < 1e21 {
        format!("{n:.0}")
    } else {
        n.to_string()
    }
}

/// `Number.prototype.toLocaleString("en-US")`: grouped, at most 3 fraction digits.
fn locale_number(n: f64) -> String {
    let fixed = format!("{:.3}", (n * 1000.0).round() / 1000.0);
    let (int, frac) = fixed.split_once('.').unwrap_or((&fixed, ""));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let frac = frac.trim_end_matches('0');
    if frac.is_empty() {
        grouped
    } else {
        format!("{grouped}.{frac}")
    }
}

fn fixed(n: f64, digits: i32) -> String {
    let scale = 10f64.powi(digits);
    format!("{:.*}", digits as usize, (n * scale).round() / scale)
}

pub fn format_bytes(bytes: f64) -> String {
    if bytes >= 1024.0 * 1024.0 {
        format!("{} MB", fixed(bytes / (1024.0 * 1024.0), 1))
    } else if bytes >= 1024.0 {
        format!("{} KB", fixed(bytes / 1024.0, 1))
    } else {
        format!("{} B", js_number(bytes))
    }
}

pub fn status_text(max_depth: f64, max_bytes: f64, scan: Option<&Diagnostic>) -> String {
    let mut lines = vec!["**pi-context-include**".to_owned(), String::new()];
    lines.push(format!("maxDepth: {}", js_number(max_depth)));
    lines.push(format!(
        "maxBytes: {} ({} KB)",
        locale_number(max_bytes),
        fixed(max_bytes / 1024.0, 0)
    ));
    let Some(s) = scan else {
        lines.push(String::new());
        lines.push(
            "No scan data yet — includes are resolved each agent turn. Send a message first."
                .into(),
        );
        return lines.join("\n");
    };
    lines.push(String::new());
    lines.push(format!("**Allowed roots** ({})", s.allowed_roots.len()));
    lines.extend(s.allowed_roots.iter().map(|r| format!("  {r}")));
    if !s.denied_roots.is_empty() {
        lines.push(format!("**Denied roots** ({})", s.denied_roots.len()));
        lines.extend(s.denied_roots.iter().map(|r| format!("  {r}")));
    }
    lines.push(String::new());
    lines.push(format!("**Context files** ({})", s.context_files.len()));
    for cf in &s.context_files {
        let status = match (&cf.error, cf.exists) {
            (Some(err), _) => format!("ERROR: {err}"),
            (None, true) => "ok".into(),
            (None, false) => "not found".into(),
        };
        lines.push(format!("  {status}  {}", cf.path));
    }
    lines.push(String::new());
    lines.push(format!(
        "**Included** ({} files, {})",
        s.included.len(),
        format_bytes(s.total_bytes as f64)
    ));
    for (path, bytes) in &s.included {
        lines.push(format!("  {}  {path}", format_bytes(*bytes as f64)));
    }
    if !s.skipped.is_empty() {
        lines.push(String::new());
        lines.push(format!("**Skipped** ({})", s.skipped.len()));
        for sk in &s.skipped {
            lines.push(format!(
                "  {}  @{}  → {}",
                sk.reason, sk.reference, sk.resolved_path
            ));
        }
    }
    if !s.included.is_empty() && s.total_bytes as f64 >= s.max_bytes {
        lines.push(String::new());
        lines.push(format!(
            "⚠️  Total ({}) is at or above the {} limit — further files were skipped.",
            format_bytes(s.total_bytes as f64),
            format_bytes(s.max_bytes)
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_match_javascript() {
        assert_eq!(js_number(10.0), "10");
        assert_eq!(js_number(1.5), "1.5");
        assert_eq!(locale_number(500_000.0), "500,000");
        assert_eq!(locale_number(1234.5), "1,234.5");
        assert_eq!(locale_number(12.0), "12");
        assert_eq!(fixed(500_000.0 / 1024.0, 0), "488");
        assert_eq!(format_bytes(512.0), "512 B");
        assert_eq!(format_bytes(2048.0), "2.0 KB");
        assert_eq!(format_bytes(3.5 * 1024.0 * 1024.0), "3.5 MB");
    }

    #[test]
    fn status_before_first_scan() {
        let text = status_text(10.0, 500_000.0, None);
        assert_eq!(
            text,
            "**pi-context-include**\n\nmaxDepth: 10\nmaxBytes: 500,000 (488 KB)\n\nNo scan data yet — includes are resolved each agent turn. Send a message first."
        );
    }
}
