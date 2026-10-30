//! `context_budget` text, `/unipi:compact-doctor` checks, and the help text.

use serde_json::Value;

use crate::config::{Config, Method};

pub fn context_budget_text(usage: Option<&Value>, config: &Config) -> String {
    let Some(tokens) = usage.and_then(|u| u["tokens"].as_f64()) else {
        return "Context budget: unknown (no token data available yet).".into();
    };
    let window = usage
        .and_then(|u| u["contextWindow"].as_f64())
        .unwrap_or(200_000.0);
    let used = tokens.max(0.0);
    let remaining = (window - used).max(0.0) as u64;
    let percent = if window > 0.0 {
        (used / window * 100.0).round()
    } else {
        0.0
    };
    let advice = if config.trigger_percent {
        let threshold = config.threshold_percent;
        if percent >= threshold {
            format!(
                "Context will be compacted automatically at the next turn boundary (threshold {threshold}%). Keep working."
            )
        } else {
            format!("Context compacts automatically at {threshold}%. Keep working.")
        }
    } else if percent >= 85.0 {
        "Context is nearly full; Pi compacts automatically near the limit and the work continues. Keep working.".into()
    } else {
        "Plenty of room. Pi compacts automatically near the limit.".into()
    };
    format!(
        "Context: ~{percent}% full (estimated {} tokens remaining)\nAdvice: {advice}",
        group_thousands(remaining)
    )
}

fn group_thousands(n: u64) -> String {
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

/// Doctor lines: `(healthy, text)`. `pi_auto` is `compaction.enabled` of pi's settings.
pub fn doctor(
    config: &Config,
    has_model: bool,
    pi_auto: Option<bool>,
    legacy_db: Option<(String, f64)>,
) -> (bool, String) {
    let mut checks: Vec<(&str, &str, String)> = Vec::new();
    let method = if config.method == Method::Vcc {
        "lossless"
    } else {
        "model summary"
    };
    let when = if config.trigger_percent {
        format!("{}% of context", config.threshold_percent)
    } else {
        "Pi's context limit".into()
    };
    let pi_compact = config.pi_compact.map_or("follow", Method::as_str);
    checks.push((
        "pass",
        "Settings",
        format!("method: {method} · when: {when} · Pi's /compact: {pi_compact}"),
    ));
    if !config.trigger_percent && pi_auto == Some(false) {
        let text = "compaction.enabled is false in pi's settings, so nothing compacts automatically. Enable it or set trigger to \"percent\".";
        checks.push(("fail", "Pi auto-compaction", text.into()));
    } else {
        let text = if pi_auto.is_none() {
            "Pi settings unreadable (defaults apply)"
        } else {
            "enabled"
        };
        checks.push(("pass", "Pi auto-compaction", text.into()));
    }
    if config.method == Method::Llm && !has_model {
        checks.push((
            "warn",
            "Model",
            "model summaries need a selected model".into(),
        ));
    }
    if let Some((dir, mb)) = legacy_db {
        checks.push((
            "warn",
            "Old continuity database",
            format!("{dir} ({mb:.1} MB) is no longer used and can be deleted."),
        ));
    }
    let healthy = checks.iter().all(|(status, _, _)| *status != "fail");
    let mut lines = vec![
        if healthy {
            "Compactor: all checks passed"
        } else {
            "Compactor: issues found"
        }
        .to_owned(),
    ];
    for (status, name, message) in checks {
        let icon = match status {
            "pass" => "✓",
            "warn" => "!",
            _ => "✗",
        };
        lines.push(format!("{icon} {name}: {message}"));
    }
    (healthy, lines.join("\n"))
}

pub const HELP: &str = "Compactor
  /unipi:compact-vcc [keep:N]    lossless compaction now (no model call)
  /unipi:compact-by-llm [focus]  compact now with a model-written summary
  /unipi:compact-then [prompt]   compact, then send the prompt
  /unipi:session-recall <query>  search everything in this session, including compacted parts
  /unipi:compact-stats           this session's savings
  /unipi:compact-doctor          check settings

Settings: <agentDir>/ext/compactor.json and <cwd>/.pi/ext/compactor.json (method, piCompact, trigger, thresholdPercent).";
