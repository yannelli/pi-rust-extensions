//! `/tool-results [status|preview|stats|reset]`; no argument opens the panel.

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

use crate::config::{Config, FIELDS};
use crate::lines::{group, short};
use crate::results::{list_text, purge, store_of};
use crate::stats::{Stats, sum};
use crate::{Shared, lock, reload, ui};

const SUBCOMMANDS: [(&str, &str); 8] = [
    ("status", "Settings and rules in effect"),
    ("preview", "The last result after the transforms"),
    ("stats", "Characters saved per tool"),
    ("list", "Stored results, newest first"),
    ("purge session", "Remove the stored results of this session"),
    ("purge all", "Remove every stored result"),
    ("purge ", "Remove one stored result: purge <result_id>"),
    ("reset", "Clear the saved stats"),
];

pub fn complete(prefix: &str) -> Option<Vec<CompletionItem>> {
    let items: Vec<CompletionItem> = SUBCOMMANDS
        .iter()
        .filter(|(name, _)| name.starts_with(prefix.trim()))
        .map(|(name, what)| CompletionItem::new(*name).description(*what))
        .collect();
    (!items.is_empty()).then_some(items)
}

pub fn status_text(config: &Config) -> String {
    let mut out = vec![format!(
        "tool-results{}",
        if config.dry_run {
            " (dry run: results are measured, not changed)"
        } else {
            ""
        }
    )];
    for field in FIELDS {
        out.push(format!("  {}: {}", field.key, config.get(field.key)));
    }
    if config.rules.is_empty() {
        out.push("Rules: none".into());
    } else {
        out.push("Rules (later rules win):".into());
        for rule in &config.rules {
            let tag = if rule.builtin { " (built-in)" } else { "" };
            out.push(format!(
                "  {}{tag}: {}",
                rule.pattern,
                Value::Object(rule.overrides.clone())
            ));
        }
    }
    out.join("\n")
}

pub fn stats_text(stats: &Stats) -> String {
    if stats.total.is_empty() {
        return "tool-results: no tool results yet.".into();
    }
    let mut out = vec!["tool-results: characters saved (this session / total)".to_owned()];
    let mut tools: Vec<_> = stats.total.iter().collect();
    tools.sort_by_key(|(_, s)| std::cmp::Reverse(s.saved()));
    for (name, total) in tools {
        let session = stats.session.get(name).map_or(0, |s| s.saved());
        out.push(format!(
            "  {name}: {} / {} over {} calls, {} stored",
            short(session as usize),
            short(total.saved() as usize),
            group(total.calls as usize),
            group(total.truncated as usize)
        ));
    }
    let (now, all) = (sum(&stats.session), sum(&stats.total));
    out.push(format!(
        "  all tools: {} / {}",
        short(now.saved() as usize),
        short(all.saved() as usize)
    ));
    if all.dry_saved > 0 {
        out.push(format!(
            "  dry run would save: {}",
            short(all.dry_saved as usize)
        ));
    }
    out.join("\n")
}

pub fn preview_text(stats: &Stats) -> String {
    let Some(last) = &stats.last else {
        return "tool-results: no result was changed yet.".into();
    };
    let mode = if last.dry {
        " (dry run, not applied)"
    } else {
        ""
    };
    format!(
        "tool-results: last {} result, {} → {} characters{mode} [{}]\n\n{}",
        last.tool,
        group(last.before),
        group(last.after),
        last.applied.join(", "),
        last.text
    )
}

pub async fn run(state: Shared, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host();
    reload(&state, host, ctx.snapshot()).await;
    let text = match args.trim() {
        "" if host.has_ui() => return ui::run::open(state, &ctx).await,
        "" | "status" => status_text(&lock(&state).config),
        "preview" => preview_text(&lock(&state).stats),
        "stats" => stats_text(&lock(&state).stats),
        "list" => list_text(&store_of(host)),
        "purge" => "tool-results: use purge session, purge all, or purge <result_id>.".into(),
        other if other.starts_with("purge ") => {
            let what = other["purge ".len()..].trim();
            if what == "all" && host.has_ui() {
                let sure = host.confirm("Purge stored tool results", "Remove every stored result?");
                if !sure.await? {
                    return host.notify_info("tool-results: nothing removed.").await;
                }
            }
            let removed = purge(&store_of(host), what, ctx.snapshot());
            format!(
                "tool-results: removed {} stored results.",
                group(removed.len())
            )
        }
        "reset" => {
            let mut session = lock(&state);
            session.stats.reset();
            let _ = session.stats.save(&crate::stats::path(&host.agent_dir()));
            "tool-results: stats cleared.".into()
        }
        other => format!(
            "tool-results: unknown argument {other:?}; use status, preview, stats, list, purge, or reset."
        ),
    };
    host.notify_info(text).await
}
