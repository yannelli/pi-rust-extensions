use std::future::Future;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::Value;

/// Session operations the pipeline needs; `Host` implements it, tests use a fake.
pub trait MemoryHost: Send + Sync {
    fn branch(&self) -> impl Future<Output = Result<Vec<Value>, String>> + Send;
    fn append(
        &self,
        custom_type: &str,
        data: Value,
    ) -> impl Future<Output = Result<(), String>> + Send;
    fn notify(&self, message: String, level: &str) -> impl Future<Output = ()> + Send;
    fn context_tokens(&self) -> impl Future<Output = Option<u64>> + Send;
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmptyBackoff {
    pub session_identity: Option<String>,
    pub coverage_id: Option<String>,
    pub tokens_at_empty: u64,
}

#[derive(Debug, Clone, Default)]
pub struct RunState {
    pub phase: Option<&'static str>,
    pub last_observer_error: Option<String>,
    pub last_reflector_error: Option<String>,
    pub last_dropper_error: Option<String>,
    pub observer_empty_backoff: Option<EmptyBackoff>,
    pub reflector_backoff: Option<EmptyBackoff>,
}

/// Process-wide flags and run state, shared by handlers and background tasks.
#[derive(Default)]
pub struct Runtime {
    pub consolidation_in_flight: AtomicBool,
    pub compact_in_flight: AtomicBool,
    pub compact_hook_in_flight: AtomicBool,
    pub project_untrusted_notified: AtomicBool,
    pub state: Mutex<RunState>,
}

impl Runtime {
    pub fn state(&self) -> std::sync::MutexGuard<'_, RunState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Sets `flag` when it was clear; the returned guard clears it on drop.
    pub fn claim<'a>(&'a self, flag: &'a AtomicBool) -> Option<FlagGuard<'a>> {
        flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| FlagGuard(flag))
    }

    /// Compaction from `agent_settled` or the `session_before_compact` hook is running.
    pub fn compacting(&self) -> bool {
        self.compact_in_flight.load(Ordering::SeqCst)
            || self.compact_hook_in_flight.load(Ordering::SeqCst)
    }

    pub fn consolidating(&self) -> bool {
        self.consolidation_in_flight.load(Ordering::SeqCst)
    }

    pub fn begin_consolidation(&self) {
        let mut state = self.state();
        state.phase = None;
        state.last_observer_error = None;
        state.last_reflector_error = None;
        state.last_dropper_error = None;
    }

    pub fn set_phase(&self, phase: Option<&'static str>) {
        self.state().phase = phase;
    }

    pub fn record_stage_error(&self, phase: &str, message: &str) {
        let mut state = self.state();
        let slot = match phase {
            "observer" => &mut state.last_observer_error,
            "reflector" => &mut state.last_reflector_error,
            _ => &mut state.last_dropper_error,
        };
        *slot = Some(message.to_owned());
    }
}

pub struct FlagGuard<'a>(&'a AtomicBool);

impl Drop for FlagGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}
