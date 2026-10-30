//! `session_before_compact` (method selection, vcc plan, model summary) and `session_compact` (card).

use pi_ext_sdk::prelude::*;

use crate::config::Method;
use crate::llm;
use crate::plan::{COMPACTOR_ID, PlanInput, plan_lossless};
use crate::rank::FileOps;
use crate::source::parse_compaction_instructions;
use crate::state::{Shared, after_compaction, debug_dump, load_config, send_prompt};
use crate::stats::{CARD_TYPE, Card};
use crate::text::str_of;

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn join_instructions(parts: &[Option<&str>]) -> Option<String> {
    let text: Vec<&str> = parts
        .iter()
        .flatten()
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .collect();
    (!text.is_empty()).then(|| text.join("\n\n"))
}
pub async fn on_before_compact(ev: EventCtx, shared: Shared) -> Result<EventReply> {
    let host = ev.host().clone();
    let config = load_config(&host, ev.snapshot());
    let reason = ev
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if reason == "overflow" {
        return Ok(EventReply::none());
    }
    let custom = ev
        .get("customInstructions")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let parsed = parse_compaction_instructions(custom.as_deref());
    let (method, from_command, active_work) = {
        let mut state = shared.lock();
        let pending = state.pending_method.take();
        let method = pending.unwrap_or(if parsed.is_compactor {
            Method::Vcc
        } else if reason == "manual" {
            config.pi_compact.unwrap_or(config.method)
        } else {
            config.method
        });
        let from_command = pending.is_some() && state.command_compaction;
        state.last_reason = Some(reason.clone());
        state.last_command = if from_command {
            state.pending_command.take()
        } else {
            None
        };
        state.pending_command = None;
        state.last_details = None;
        state.pending_follow_up = None;
        (method, from_command, state.active_work())
    };
    let preparation = ev.get("preparation").cloned().unwrap_or(Value::Null);

    if method == Method::Llm {
        let user = if parsed.is_compactor {
            None
        } else {
            custom.as_deref()
        };
        let focus = join_instructions(&[user, Some(&config.llm_instructions)]);
        let split = preparation["isSplitTurn"] == true
            && preparation["turnPrefixMessages"]
                .as_array()
                .is_some_and(|m| !m.is_empty());
        let messages = preparation["messagesToSummarize"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if split || messages.is_empty() {
            return Ok(EventReply::none());
        }
        let previous = preparation["previousSummary"].as_str();
        return match llm::summarize(&host, &messages, previous, focus.as_deref()).await {
            Ok((text, usage)) => {
                shared.lock().last_stats = None;
                let summary = llm::with_active_work(
                    &format!("{text}{}", llm::file_ops_tail(&preparation["fileOps"])),
                    &active_work,
                );
                let compaction = json!({
                    "summary": summary,
                    "firstKeptEntryId": preparation["firstKeptEntryId"],
                    "tokensBefore": preparation["tokensBefore"],
                    "usage": usage,
                    "details": { "compactor": COMPACTOR_ID, "version": 2, "method": "llm" },
                });
                Ok(EventReply::result(json!({ "compaction": compaction })))
            }
            Err(_) if ev.is_cancelled() => Ok(EventReply::cancel()),
            Err(err) => {
                debug_dump(
                    &host,
                    &config,
                    json!({ "method": "llm", "fallback": true, "error": err.message() }),
                );
                Ok(EventReply::none())
            }
        };
    }

    let file_ops = &preparation["fileOps"];
    let input = PlanInput {
        branch: ev
            .get("branchEntries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default(),
        tokens_before: preparation["tokensBefore"].as_u64(),
        previous_summary: preparation["previousSummary"].as_str().map(str::to_owned),
        file_ops: FileOps {
            read: strings(&file_ops["read"]),
            modified: [strings(&file_ops["written"]), strings(&file_ops["edited"])].concat(),
        },
        keep: Some(parsed.keep),
        keep_explicit: parsed.keep_explicit,
        context_window: ev.snapshot().model.as_ref().and_then(|m| m.context_window),
        config: config.clone(),
        cwd: Some(ev.snapshot().cwd.clone()).filter(|c| !c.is_empty()),
        reason: Some(reason.clone()).filter(|r| !r.is_empty()),
        active_work,
    };
    let plan = match plan_lossless(&input) {
        Ok(plan) => plan,
        Err(why) => {
            if reason == "manual" || from_command {
                let text = if why == "no_live_messages" {
                    "compactor: nothing to compact"
                } else {
                    "compactor: too few messages to compact"
                };
                host.notify_warning(text).await.ok();
            }
            debug_dump(&host, &config, json!({ "cancelled": true, "reason": why }));
            return Ok(EventReply::cancel());
        }
    };
    {
        let mut state = shared.lock();
        state.last_stats = Some(plan.stats.clone());
        state.last_details = Some(plan.details.clone());
        if !parsed.is_compactor && reason == "manual" {
            state.pending_follow_up = parsed.follow_up.clone();
        }
    }
    let preview: String = plan.summary.chars().take(800).collect();
    debug_dump(
        &host,
        &config,
        json!({ "method": "vcc", "reason": reason, "details": plan.details, "summaryPreview": preview }),
    );
    let compaction = json!({
        "summary": plan.summary,
        "details": plan.details,
        "tokensBefore": preparation["tokensBefore"],
        "firstKeptEntryId": plan.first_kept_entry_id,
    });
    Ok(EventReply::result(json!({ "compaction": compaction })))
}

pub async fn on_session_compact(ev: EventCtx, shared: Shared) -> Result<EventReply> {
    let host = ev.host().clone();
    let entry = ev.get("compactionEntry").cloned().unwrap_or(Value::Null);
    let details = &entry["details"];
    let ours = str_of(details, "compactor") == COMPACTOR_ID;
    let method = if ours && str_of(details, "method") != "llm" {
        Method::Vcc
    } else {
        Method::Llm
    };
    let (stats, last_details, was_command, follow_up, reason, command) = {
        let mut state = shared.lock();
        let was_command = std::mem::take(&mut state.command_compaction);
        let stats = state.last_stats.clone().filter(|_| ours);
        let last_details = state.last_details.clone().filter(|_| ours);
        (
            stats,
            last_details,
            was_command,
            state.pending_follow_up.take(),
            state.last_reason.clone(),
            state.last_command.clone(),
        )
    };
    after_compaction(&host, &shared, stats.as_ref(), method).await;
    if was_command || load_config(&host, ev.snapshot()).notify {
        let trigger = reason
            .as_deref()
            .filter(|r| matches!(*r, "threshold" | "overflow"))
            .unwrap_or("manual");
        let tokens_before = entry["tokensBefore"]
            .as_u64()
            .or(stats.as_ref().and_then(|s| s.tokens_before))
            .unwrap_or(0);
        let card = Card {
            method,
            trigger,
            command: command.as_deref().filter(|_| was_command),
            tokens_before,
            stats: stats.as_ref(),
            details: last_details.as_ref(),
            summary: entry["summary"].as_str(),
            percent: None,
            threshold: None,
        };
        if let Err(err) = host.append_entry(CARD_TYPE, &card.data()).await {
            pi_ext_sdk::debug!("card append failed: {}", err.message());
        }
    }
    if let Some(prompt) = follow_up {
        tokio::spawn(async move { send_prompt(&host, &prompt, DeliverAs::FollowUp).await });
    }
    Ok(EventReply::none())
}
