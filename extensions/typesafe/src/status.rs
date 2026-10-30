use pi_ext_sdk::NotifyType;

use crate::auth_state::Level;
use crate::backends::{BACKEND_ENV, DEFAULT_BACKEND, MODEL_ENV};
use crate::client::DEFAULT_MAX_REQUESTS;
use crate::state::State;

/// The upstream `/typesafe status` line: gate, route, key, session, today, caps, consent.
pub fn status_text(state: &State) -> (String, NotifyType) {
    let spend = state.client().map(|client| client.spend());
    let (level, auth) = state.describe_auth();
    let (consent, names_model) = state.consent();
    let session = match &spend {
        Some(s) => format!(
            "Session {}/{DEFAULT_MAX_REQUESTS} attempts, {} successful, {} failed, {} input tokens (~${:.4}).",
            s.session.requests_started,
            s.session.requests_succeeded,
            s.session.requests_failed,
            s.session.input_tokens,
            s.session.usd
        ),
        None => {
            format!("Session 0/{DEFAULT_MAX_REQUESTS} attempts; no client yet in this session.")
        }
    };
    let today = spend.as_ref().map_or(String::new(), |s| {
        let t = &s.today.totals;
        format!(
            "Today {} requests ({} ok, {} failed), {} input tokens, ~${:.4}.",
            t.requests_started,
            t.requests_succeeded,
            t.requests_failed,
            t.input_tokens,
            s.today.estimated_usd
        )
    });
    let blocked = spend.as_ref().and_then(|s| s.blocked.as_ref()).map_or(String::new(), |b| {
        format!("Cap reached: {} {}/{} on {}; no request will be submitted until the local day rolls over.", b.cap, b.used, b.limit, b.day)
    });
    let backend = &state.backend;
    let route = match &state.backend_error {
        Some(error) => {
            format!("Backend: {BACKEND_ENV} is invalid — {error} Judgments are skipped.")
        }
        None => format!(
            "{} backend: {}, key from {}.",
            backend.label,
            backend.host_name(),
            backend.key_env
        ),
    };
    let version = match state.backend_error.is_none() && !names_model {
        true => format!("Model: {}.", backend.sent_model(state.model.as_deref())),
        false => String::new(),
    };
    let model_hint = match backend.name == DEFAULT_BACKEND {
        true => String::new(),
        false => format!("Set {MODEL_ENV} to send another model."),
    };
    let enabled = state.enabled();
    let parts = [
        format!("TypeSafe: {}.", if enabled { "enabled" } else { "disabled" }),
        route,
        auth,
        session,
        today,
        blocked,
        version,
        model_hint,
        "Session limits reset on session start/reload; daily counters persist and caps come from client options or PI_TYPESAFE_MAX_* environment variables.".to_owned(),
        if state.backend_error.is_none() { consent } else { String::new() },
    ];
    let text = parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let warn = (level == Level::Error && enabled) || state.backend_error.is_some();
    (
        text,
        if warn {
            NotifyType::Warning
        } else {
            NotifyType::Info
        },
    )
}
