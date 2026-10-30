use std::time::{SystemTime, UNIX_EPOCH};

use pi_ext_sdk::style;

use super::render::{accent, muted};
use super::{DetailAction, Row};
use crate::trust::LastRun;

const LABEL: usize = 10;

/// The details view of the row with `hash`; `action` marks the chosen action.
pub fn details_lines(rows: &[Row], hash: &str, action: Option<usize>, width: usize) -> Vec<String> {
    let Some(row) = rows.iter().find(|r| r.def.hash == hash) else {
        return Vec::new();
    };
    let def = &row.def;
    let title = format!(" {} · {}", def.event, row.matcher());
    let mut lines = vec![accent(&style::bold(&title))];
    let mut field = |label: &str, value: &str| {
        if value.is_empty() {
            return;
        }
        let head = muted(&style::fit(label, LABEL));
        if width >= 40 {
            let body = wrap(value, width.saturating_sub(LABEL + 1));
            lines.push(format!(" {head}{}", body[0]));
            let pad = " ".repeat(LABEL + 1);
            lines.extend(body[1..].iter().map(|l| format!("{pad}{l}")));
        } else {
            lines.push(format!(" {head}"));
            let body = wrap(value, width.saturating_sub(3));
            lines.extend(body.iter().map(|l| format!("   {l}")));
        }
    };
    let source = format!("{}, {} scope", row.group.title(), row.scope_label());
    field("Source", &source);
    field("File", &def.file.to_string_lossy());
    if let Some(id) = def.plugin.as_ref().and_then(|p| p.id.as_deref()) {
        field("Plugin", id);
    }
    field("Type", def.kind());
    let command_label = match def.kind() {
        "http" => "URL",
        "prompt" => "Prompt",
        _ => "Command",
    };
    field(command_label, &def.summary());
    let default = if def.handler.get("timeout").is_some() {
        ""
    } else {
        " (default)"
    };
    field("Timeout", &format!("{}s{default}", def.timeout().as_secs()));
    let flags: Vec<&str> = ["async", "asyncRewake", "once"]
        .into_iter()
        .filter(|f| def.bool_field(f))
        .collect();
    field("Flags", &flags.join(", "));
    field("Status", def.str_field("statusMessage").unwrap_or_default());
    field("Trust", row.trust.name());
    field("State", if row.enabled { "enabled" } else { "disabled" });
    let last = row.last.as_ref().map_or("never run".into(), run_text);
    field("Last run", &last);
    if let Some(run) = &row.last {
        for (label, out) in [("stdout", &run.stdout), ("stderr", &run.stderr)] {
            let text: Vec<&str> = out
                .lines()
                .filter(|l| !l.trim().is_empty())
                .take(4)
                .collect();
            field(label, &text.join(" ⏎ "));
        }
    }
    if let Some(action) = action {
        lines.push(String::new());
        lines.push(muted(" Actions"));
        for (index, item) in DetailAction::ALL.iter().enumerate() {
            let label = item.label(row.enabled);
            lines.push(if index == action {
                format!(" {}", accent(&format!("→ {label}")))
            } else {
                format!("   {label}")
            });
        }
    }
    lines
}

pub fn run_text(run: &LastRun) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let age = now.saturating_sub(run.at);
    let ago = match age {
        0..60 => format!("{age}s ago"),
        60..3600 => format!("{}m ago", age / 60),
        3600..86400 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86400),
    };
    let exit = run
        .exit
        .map_or("no exit".to_owned(), |c| format!("exit {c}"));
    format!("{exit} {} {}ms {ago}", run.result, run.duration_ms)
}

/// Splits plain text into lines of at most `width` columns.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = style::char_width(c);
        if used + w > width && used > 0 {
            lines.push(String::new());
            used = 0;
        }
        lines.last_mut().expect("one line").push(c);
        used += w;
    }
    lines
}
