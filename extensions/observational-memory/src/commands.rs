use std::sync::atomic::Ordering;

use serde_json::Value;

use crate::agents::completion::locale;
use crate::agents::coverage::pool_metrics;
use crate::config::{Config, resolve_compact_after_tokens};
use crate::ledger::*;
use crate::runtime::Runtime;

fn pct(current: u64, total: u64) -> u64 {
    if total == 0 {
        0
    } else {
        ((current as f64 / total as f64) * 100.0).round() as u64
    }
}

fn with_suffixes(line: String, suffixes: &[(char, usize)]) -> String {
    let rendered: Vec<String> = suffixes
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(sign, n)| format!("{sign}{}", locale(*n as u64)))
        .collect();
    if rendered.is_empty() {
        line
    } else {
        format!("{line} {}", rendered.join(" "))
    }
}

/// The `/om:status` text.
pub fn status_text(
    entries: &[Value],
    config: &Config,
    runtime: &Runtime,
    context_window: Option<u64>,
) -> String {
    let folded = fold_ledger(entries, None);
    let visible = visible_projection(entries, None);
    let full = full_projection(entries, None);
    let drift = diff_projection(&visible, &full);
    let visible_obs_tokens: u64 = visible.observations.iter().map(|o| o.token_count).sum();
    let visible_ref_tokens: u64 = visible.reflections.iter().map(|r| r.token_count).sum();
    let target = config.observations_pool_target_tokens;
    let active = pool_metrics(&folded.active_observations, target);
    let observation_line = with_suffixes(
        format!(
            "Observations: {} recorded / {} dropped / {} active / {} visible",
            folded.observations.len(),
            folded.dropped_observation_ids.len(),
            folded.active_observations.len(),
            visible.observations.len()
        ),
        &[
            ('+', drift.observations_only_in_full.len()),
            ('-', drift.dropped_only_in_full.len()),
        ],
    );
    let reflection_line = with_suffixes(
        format!(
            "Reflections:  {} recorded / {} visible",
            folded.reflections.len(),
            visible.reflections.len()
        ),
        &[('+', drift.reflections_only_in_full.len())],
    );
    let obs = raw_tokens_since_observation_coverage(entries);
    let refl = raw_tokens_since_reflection_coverage(entries);
    let compaction = raw_tokens_since_last_compaction(entries);
    let threshold = resolve_compact_after_tokens(config, context_window);
    let (observe, reflect, pool_max) = (
        config.observe_after_tokens,
        config.reflect_after_tokens,
        config.observations_pool_max_tokens,
    );

    let mut lines: Vec<String> = Vec::new();
    if config.passive {
        lines.extend([
            "── Mode ──".to_owned(),
            "Passive: automatic memory workers and auto-compaction disabled; manual/Pi compaction, commands, and recall remain active".to_owned(),
            String::new(),
        ]);
    }
    lines.extend([
        "── Memory ──".to_owned(),
        observation_line,
        reflection_line,
        String::new(),
        "── Activity ──".to_owned(),
        format!(
            "Next observation: ~{} / {} tokens ({}%)",
            locale(obs),
            locale(observe),
            pct(obs, observe)
        ),
        format!(
            "Next reflection:  ~{} / {} tokens ({}%)",
            locale(refl),
            locale(reflect),
            pct(refl, reflect)
        ),
        format!(
            "Next compaction:  ~{} / {} estimated source tokens ({}%)",
            locale(compaction),
            locale(threshold),
            pct(compaction, threshold)
        ),
        format!(
            "Visible observation pool: ~{} / {} tokens ({}%)",
            locale(visible_obs_tokens),
            locale(pool_max),
            pct(visible_obs_tokens, pool_max)
        ),
        format!(
            "Active observation pool: ~{} / {} target tokens ({}%)",
            locale(active.observation_tokens),
            locale(target),
            pct(active.observation_tokens, target)
        ),
        format!(
            "Reflection pool:         ~{} tokens",
            locale(visible_ref_tokens)
        ),
    ]);

    let consolidation = runtime.consolidation_in_flight.load(Ordering::Acquire);
    let compact = runtime.compact_in_flight.load(Ordering::Acquire);
    let hook = runtime.compact_hook_in_flight.load(Ordering::Acquire);
    let state = runtime.state();
    if consolidation || compact || hook {
        lines.extend([String::new(), "── In flight ──".to_owned()]);
        if consolidation {
            let phase = state.phase.map(|p| format!(" ({p})")).unwrap_or_default();
            lines.push(format!("Consolidation: running{phase}"));
        }
        if compact {
            lines.push("Auto-compaction: running".to_owned());
        }
        if hook {
            lines.push("Compaction hook: running".to_owned());
        }
    }
    let errors = [
        ("Observer", &state.last_observer_error),
        ("Reflector", &state.last_reflector_error),
        ("Dropper", &state.last_dropper_error),
    ];
    if errors.iter().any(|(_, e)| e.is_some()) {
        lines.extend([String::new(), "── Last error ──".to_owned()]);
        for (label, error) in errors {
            if let Some(error) = error {
                lines.push(format!("{label}: {error}"));
            }
        }
    }
    lines.join("\n")
}

fn render_list(items: Vec<String>, empty: String) -> String {
    if items.is_empty() {
        empty
    } else {
        items.join("\n")
    }
}

/// The `/om:view` text: `None` for an unknown mode.
pub fn view_text(entries: &[Value], args: &str) -> Option<String> {
    let mode = args.split_whitespace().next();
    let (projection, scope) = match mode {
        Some("full") => (full_projection(entries, None), "recorded"),
        None | Some("visible") => (visible_projection(entries, None), "visible"),
        Some(_) => return None,
    };
    Some(
        [
            "── Reflections ──".to_owned(),
            render_list(
                projection
                    .reflections
                    .iter()
                    .map(reflection_to_summary_line)
                    .collect(),
                format!("No {scope} reflections."),
            ),
            String::new(),
            "── Observations ──".to_owned(),
            render_list(
                projection
                    .observations
                    .iter()
                    .map(observation_to_summary_line)
                    .collect(),
                format!("No {scope} observations."),
            ),
        ]
        .join("\n"),
    )
}
