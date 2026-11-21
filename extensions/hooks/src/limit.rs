//! Claude Code's limit on prompt and agent hooks (2.1.302 and 2.1.312), behind [`SWITCH`].

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::{Instant, sleep_until};

use crate::config::options::Options;
use crate::model::HookDef;

pub const SWITCH: &str = "limits/promptAgentHooks";
const EVENT_LIMIT: usize = 12;
const SESSION_LIMIT: usize = 24;
const OVERFLOW_LIMIT: usize = 12;

/// Session slots: 24 running hooks, plus 12 for hooks that waited longer than their timeout.
#[derive(Clone)]
pub struct SessionSlots {
    running: Arc<Semaphore>,
    overflow: Arc<Semaphore>,
}

impl Default for SessionSlots {
    fn default() -> Self {
        Self {
            running: Arc::new(Semaphore::new(SESSION_LIMIT)),
            overflow: Arc::new(Semaphore::new(OVERFLOW_LIMIT)),
        }
    }
}

impl SessionSlots {
    async fn acquire(&self, overflow_at: Instant) -> Option<OwnedSemaphorePermit> {
        let running = self.running.clone().acquire_owned();
        tokio::pin!(running);
        tokio::select! {
            biased;
            permit = &mut running => return permit.ok(),
            () = sleep_until(overflow_at) => {}
        }
        tokio::select! {
            biased;
            permit = running => permit.ok(),
            permit = self.overflow.clone().acquire_owned() => permit.ok(),
        }
    }
}

/// The slots that the sync prompt and agent hooks of one event share.
#[derive(Clone)]
pub struct Gate {
    event: Arc<Semaphore>,
    session: SessionSlots,
}

impl Gate {
    /// A gate for one event, or `None` when [`SWITCH`] is off.
    pub fn new(options: Option<&Options>, session: &SessionSlots) -> Option<Self> {
        options.is_some_and(|o| o.switch_on(SWITCH)).then(|| Self {
            event: Arc::new(Semaphore::new(EVENT_LIMIT)),
            session: session.clone(),
        })
    }
}

/// Runs a sync `hook`. A prompt or agent hook first waits for an event slot and a session slot.
pub async fn run<T>(gate: Option<&Gate>, def: &HookDef, hook: impl Future<Output = T>) -> T {
    let limited = matches!(def.kind(), "prompt" | "agent");
    run_with(gate.filter(|_| limited), def.timeout(), hook).await
}

async fn run_with<T>(gate: Option<&Gate>, timeout: Duration, hook: impl Future<Output = T>) -> T {
    let Some(gate) = gate else {
        return hook.await;
    };
    let overflow_at = Instant::now() + timeout;
    let _event = gate.event.clone().acquire_owned().await.ok();
    let _session = gate.session.acquire(overflow_at).await;
    hook.await
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

    use serde_json::json;
    use tokio::task::JoinSet;

    use super::*;
    use crate::model::{Scope, Source};

    const HOLD: Duration = Duration::from_millis(300);
    const LONG_WAIT: Duration = Duration::from_secs(30);

    fn options(on: bool) -> Options {
        let mut options = Options::default();
        options.switches.insert(SWITCH, on);
        options
    }

    fn def(kind: &str) -> HookDef {
        let handler = json!({ "type": kind, "prompt": "x", "command": "x" });
        HookDef::new(
            Source::Claude,
            Scope::Global,
            "/f".into(),
            "Stop",
            None,
            handler,
        )
    }

    /// The most hooks running at once when `per_gate` hooks run through each gate.
    async fn peak(gates: &[Option<Gate>], per_gate: usize, wait: Duration) -> usize {
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = JoinSet::new();
        for gate in gates.iter().flat_map(|g| std::iter::repeat_n(g, per_gate)) {
            let (gate, running, peak) = (gate.clone(), running.clone(), peak.clone());
            tasks.spawn(async move {
                run_with(gate.as_ref(), wait, async {
                    peak.fetch_max(running.fetch_add(1, SeqCst) + 1, SeqCst);
                    tokio::time::sleep(HOLD).await;
                    running.fetch_sub(1, SeqCst);
                })
                .await;
            });
        }
        tasks.join_all().await;
        peak.load(SeqCst)
    }

    #[test]
    fn gate_follows_the_switch() {
        let slots = SessionSlots::default();
        assert!(Gate::new(None, &slots).is_none());
        assert!(Gate::new(Some(&Options::default()), &slots).is_none());
        assert!(Gate::new(Some(&options(false)), &slots).is_none());
        assert!(Gate::new(Some(&options(true)), &slots).is_some());
    }

    #[tokio::test]
    async fn switch_off_starts_all_hooks_at_once() {
        let gate = Gate::new(Some(&options(false)), &SessionSlots::default());
        assert_eq!(peak(&[gate], 30, LONG_WAIT).await, 30);
    }

    #[tokio::test]
    async fn one_event_runs_12_at_once() {
        let gate = Gate::new(Some(&options(true)), &SessionSlots::default());
        assert_eq!(peak(&[gate], 30, LONG_WAIT).await, 12);
    }

    #[tokio::test]
    async fn session_runs_24_at_once_across_events() {
        let slots = SessionSlots::default();
        let gates: Vec<_> = (0..3)
            .map(|_| Gate::new(Some(&options(true)), &slots))
            .collect();
        assert_eq!(peak(&gates, 12, LONG_WAIT).await, 24);
    }

    #[tokio::test]
    async fn hooks_past_their_timeout_use_12_more_session_slots() {
        let slots = SessionSlots::default();
        let gates: Vec<_> = (0..4)
            .map(|_| Gate::new(Some(&options(true)), &slots))
            .collect();
        let wait = Duration::from_millis(20);
        assert_eq!(peak(&gates, 12, wait).await, 36);
    }

    #[tokio::test]
    async fn run_limits_by_handler_kind() {
        let gate = Gate::new(Some(&options(true)), &SessionSlots::default());
        let mut tasks = JoinSet::new();
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        for kind in ["prompt", "agent", "command"].repeat(10) {
            let (gate, running, peak) = (gate.clone(), running.clone(), peak.clone());
            tasks.spawn(async move {
                run(gate.as_ref(), &def(kind), async {
                    peak.fetch_max(running.fetch_add(1, SeqCst) + 1, SeqCst);
                    tokio::time::sleep(HOLD).await;
                    running.fetch_sub(1, SeqCst);
                })
                .await;
            });
        }
        tasks.join_all().await;
        assert_eq!(peak.load(SeqCst), 12 + 10);
    }
}
