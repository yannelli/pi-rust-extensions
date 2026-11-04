//! Timers for the agent: `sleep`, `heartbeat`, `schedule`, and `/timers`.

mod clock;
mod command;
mod config;
mod cron;
mod runtime;
mod state;
mod tools;

#[cfg(test)]
mod protocol_tests;
#[cfg(test)]
mod test_host;
#[cfg(test)]
mod tests;

use std::process::ExitCode;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use runtime::Runtime;

const POLICY_SECTION: &str = "pi_timers_policy";
const POLICY: &str = "Timers: use sleep only to wait a fixed time; to wait for background tasks, subagents, or messages, end the turn, since their notifications wake you. Use heartbeat as a watchdog that wakes you while idle during long unattended work (ack it when done), and schedule for prompts due at a set time, after a delay, or on a repeating interval or cron.";

pub fn main() -> ExitCode {
    extension().run()
}

/// Arms the timers of the session in `snapshot`, or of the host's latest snapshot.
pub(crate) fn ensure(host: &Host, snapshot: &Snapshot, runtime: &Arc<Runtime>) {
    let session = snapshot
        .session_id
        .clone()
        .or_else(|| host.snapshot().and_then(|s| s.session_id.clone()));
    runtime.bind(host, session.as_deref());
}

/// A steering message or a prompt ends a `sleep`; a queued follow-up waits until the agent
/// stops, so it leaves the sleep running.
pub fn input_wakes_sleep(streaming_behavior: Option<&str>) -> bool {
    streaming_behavior != Some("followUp")
}

pub fn extension() -> Extension {
    let runtime = Arc::new(Runtime::default());
    let ext = Extension::new("timers");
    let ext = tools::register(ext, runtime.clone());
    let ext = command::register(ext, runtime.clone());
    let (start, input, shutdown) = (runtime.clone(), runtime.clone(), runtime);
    ext.on("before_agent_start", |_ev: EventCtx| async move {
        Ok(EventReply::section(POLICY_SECTION, POLICY))
    })
    .on("session_start", move |ev: EventCtx| {
        let runtime = start.clone();
        async move {
            ensure(ev.host(), ev.snapshot(), &runtime);
            Ok(())
        }
    })
    .on("input", move |ev: EventCtx| {
        let runtime = input.clone();
        let behavior = ev.get("streamingBehavior").and_then(Value::as_str);
        if input_wakes_sleep(behavior) {
            let _ = runtime.wake.send("user input".into());
        }
        async { Ok(()) }
    })
    .on("session_shutdown", move |_ev: EventCtx| {
        let runtime = shutdown.clone();
        async move {
            runtime.unbind();
            Ok(())
        }
    })
}
