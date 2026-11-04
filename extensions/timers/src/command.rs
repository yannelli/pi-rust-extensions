use std::sync::Arc;

use pi_ext_sdk::CompletionItem;
use pi_ext_sdk::prelude::*;

use crate::ensure;
use crate::runtime::Runtime;

pub const USAGE: &str = "usage: /timers [cancel <id> | clear]";

/// Runs `/timers` arguments; returns the text to show and whether it is an error.
pub fn run(args: &str, runtime: &Runtime) -> (String, bool) {
    let mut words = args.split_whitespace();
    match (words.next(), words.next(), words.next()) {
        (None, ..) => (runtime.read(|t| t.list_text()), false),
        (Some("cancel"), Some(id), None) => match runtime.update(|t| t.remove(id)) {
            Ok(label) => (format!("Removed {label}."), false),
            Err(err) => (err, true),
        },
        (Some("clear"), None, _) => {
            let count = runtime.update(|t| {
                let count = t.heartbeats.len() + t.schedules.len();
                t.heartbeats.clear();
                t.schedules.clear();
                count
            });
            (format!("Removed {count} timer(s)."), false)
        }
        _ => (USAGE.to_owned(), true),
    }
}

pub fn register(ext: Extension, runtime: Arc<Runtime>) -> Extension {
    let complete = runtime.clone();
    ext.command(
        "timers",
        "List heartbeats and schedules: /timers [cancel <id> | clear]",
        move |args, ctx: CommandCtx| {
            let runtime = runtime.clone();
            async move {
                ensure(ctx.host(), ctx.snapshot(), &runtime);
                let (text, error) = run(&args, &runtime);
                runtime.refresh_status(ctx.host()).await;
                let sent = if error {
                    ctx.host().notify_error(text.clone()).await
                } else {
                    ctx.host().notify_info(text.clone()).await
                };
                if let Err(err) = sent {
                    pi_ext_sdk::info!("{text} ({err})");
                }
                Ok(())
            }
        },
    )
    .completions("timers", move |prefix: String, _host: Host| {
        let runtime = complete.clone();
        async move {
            let items = match prefix.strip_prefix("cancel ") {
                Some(id) => runtime.read(|t| {
                    let beats = t.heartbeats.iter().map(|h| (h.id.clone(), h.label()));
                    let schedules = t.schedules.iter().map(|s| (s.id.clone(), s.label()));
                    beats
                        .chain(schedules)
                        .filter(|(timer, _)| timer.starts_with(id.trim()))
                        .map(|(timer, label)| {
                            CompletionItem::new(format!("cancel {timer}")).label(label)
                        })
                        .collect()
                }),
                None => ["cancel ", "clear"]
                    .iter()
                    .filter(|word| word.starts_with(prefix.as_str()))
                    .map(|word| CompletionItem::new(*word))
                    .collect(),
            };
            Ok(Some(items))
        }
    })
}
