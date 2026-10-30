//! `turn_end`: steer-queued compact-then and the percentage trigger, as boundary compaction drafts.

use pi_ext_sdk::prelude::*;

use crate::config::{Config, Method};
use crate::cut::{
    DEFAULT_CHARS_PER_TOKEN, TailBudget, apply_tail_budget, build_own_cut, collect_live,
};
use crate::llm;
use crate::plan::{COMPACTOR_ID, PlanInput, Stats, plan_lossless};
use crate::state::{
    COMPACT_THEN, QueueMode, Shared, after_compaction, load_config, now_ms, send_prompt,
};
use crate::stats::{CARD_TYPE, Card};
use crate::text::str_of;
use crate::trigger::{AutoConfig, Usage, decide, mark_complete, mark_error};

struct Draft {
    entry: Value,
    stats: Option<Stats>,
}

fn lossless_draft(
    branch: Vec<Value>,
    config: &Config,
    ev: &EventCtx,
    tokens: Option<u64>,
    active: Vec<(String, String)>,
) -> Option<Draft> {
    let input = PlanInput {
        branch,
        tokens_before: tokens,
        context_window: ev.snapshot().model.as_ref().and_then(|m| m.context_window),
        config: config.clone(),
        cwd: Some(ev.snapshot().cwd.clone()).filter(|c| !c.is_empty()),
        reason: Some("percent".into()),
        active_work: active,
        ..PlanInput::default()
    };
    let plan = plan_lossless(&input).ok()?;
    let first_kept = Some(plan.first_kept_entry_id).filter(|id| !id.is_empty());
    let entry = json!({ "type": "compaction", "summary": plan.summary, "firstKeptEntryId": first_kept, "details": plan.details });
    Some(Draft {
        entry,
        stats: Some(plan.stats),
    })
}

async fn llm_draft(
    host: &Host,
    branch: &[Value],
    config: &Config,
    active: &[(String, String)],
) -> Result<Option<Draft>> {
    let Ok(cut) = build_own_cut(branch, 1) else {
        return Ok(None);
    };
    let cut = apply_tail_budget(branch, cut, TailBudget::new(DEFAULT_CHARS_PER_TOKEN));
    if cut.summarized == 0 {
        return Ok(None);
    }
    let live = collect_live(branch);
    let messages: Vec<Value> = live[..cut.summarized.min(live.len())]
        .iter()
        .map(|l| l.message.clone())
        .collect();
    let previous = branch
        .iter()
        .rev()
        .find(|e| str_of(e, "type") == "compaction")
        .and_then(|e| e["summary"].as_str());
    let focus = Some(config.llm_instructions.as_str()).filter(|f| !f.trim().is_empty());
    let (text, usage) = llm::summarize(host, &messages, previous, focus).await?;
    let first_kept = Some(cut.first_kept_entry_id).filter(|id| !id.is_empty());
    let entry = json!({
        "type": "compaction",
        "summary": llm::with_active_work(&text, active),
        "firstKeptEntryId": first_kept,
        "details": { "compactor": COMPACTOR_ID, "version": 2, "method": "llm", "reason": "percent" },
        "usage": usage,
    });
    Ok(Some(Draft { entry, stats: None }))
}

async fn draft(
    ev: &EventCtx,
    config: &Config,
    shared: &Shared,
    tokens: Option<u64>,
) -> Result<Option<Draft>> {
    let host = ev.host();
    let branch = host.branch(None).await?;
    let active = shared.lock().active_work();
    match config.method {
        Method::Llm => llm_draft(host, &branch, config, &active).await,
        Method::Vcc => Ok(lossless_draft(branch, config, ev, tokens, active)),
    }
}

fn card(
    config: &Config,
    draft: &Draft,
    trigger: &str,
    command: Option<&str>,
    tokens: u64,
    usage: Option<(f64, f64)>,
) -> Value {
    let card = Card {
        method: config.method,
        trigger,
        command,
        tokens_before: tokens,
        stats: draft.stats.as_ref(),
        details: draft.entry.get("details"),
        summary: draft.entry["summary"].as_str(),
        percent: usage.map(|u| u.0),
        threshold: usage.map(|u| u.1),
    };
    json!({ "type": "custom", "customType": CARD_TYPE, "data": card.data() })
}

fn has_compaction(entries: &[Value]) -> bool {
    entries.iter().any(|e| str_of(e, "type") == "compaction")
}

pub async fn on_turn_end(ev: EventCtx, shared: Shared) -> Result<EventReply> {
    let host = ev.host().clone();
    let config = load_config(&host, ev.snapshot());
    let original: Vec<Value> = ev
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut entries = original.clone();
    let aborted = ev.get("outcome").and_then(Value::as_str) == Some("aborted");
    let usage_value = ev.snapshot().context_usage.clone();
    let tokens_now = usage_value.as_ref().and_then(|u| u["tokens"].as_u64());

    let queued = {
        let mut state = shared.lock();
        match state.compact_then.take() {
            Some((prompt, QueueMode::Steer)) => Some(prompt),
            other => {
                state.compact_then = other;
                None
            }
        }
    };
    if let Some(prompt) = queued.filter(|_| !aborted) {
        if !has_compaction(&entries) {
            match draft(&ev, &config, &shared, tokens_now).await {
                Ok(Some(d)) => {
                    after_compaction(&host, &shared, d.stats.as_ref(), config.method).await;
                    let tokens = d.stats.as_ref().and_then(|s| s.tokens_before).unwrap_or(0);
                    let card = config
                        .notify
                        .then(|| card(&config, &d, "manual", Some(COMPACT_THEN), tokens, None));
                    entries.push(d.entry);
                    entries.extend(card);
                }
                Ok(None) if config.notify => {
                    host.notify_warning("compactor: nothing to compact for /unipi:compact-then")
                        .await
                        .ok();
                }
                Ok(None) => {}
                Err(err) => {
                    host.notify_warning(format!("Compaction failed: {}", err.message()))
                        .await
                        .ok();
                }
            }
        }
        if !prompt.is_empty() {
            send_prompt(&host, &prompt, DeliverAs::Steer).await;
        }
    }

    if config.trigger_percent && !aborted && !has_compaction(&entries) {
        let usage = Usage::from_value(usage_value.as_ref());
        let auto = AutoConfig::of(&config);
        let decision = {
            let mut state = shared.lock();
            let decision = decide(auto, usage, &state.auto, now_ms());
            state.auto = decision.state.clone();
            decision
        };
        if decision.trigger {
            let tokens = decision.usage.map(|u| u.tokens);
            match draft(&ev, &config, &shared, tokens).await {
                Ok(Some(d)) => {
                    let state_now = shared.lock().auto.clone();
                    shared.lock().auto = mark_complete(&state_now);
                    after_compaction(&host, &shared, d.stats.as_ref(), config.method).await;
                    let percent = decision.usage.map(|u| (u.percent, auto.threshold_percent));
                    let card = config
                        .notify
                        .then(|| card(&config, &d, "percent", None, tokens.unwrap_or(0), percent));
                    entries.push(d.entry);
                    entries.extend(card);
                }
                outcome => {
                    let state_now = shared.lock().auto.clone();
                    shared.lock().auto = mark_error(&state_now, now_ms());
                    if let (Err(err), true) = (outcome, config.notify) {
                        host.notify_warning(format!("Auto-compaction failed: {}", err.message()))
                            .await
                            .ok();
                    }
                }
            }
        }
    }

    if entries == original {
        return Ok(EventReply::none());
    }
    Ok(EventReply::result(json!({ "entries": entries })))
}
