//! A scripted `RouteEnv` and builders for profiles, snapshots, and route inputs.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use pi_ext_subscription_router::config::{ModelRef, Profile, parse_config};
use pi_ext_subscription_router::router::{FailedRequest, Reason, RouteEnv, RouteInput, Router};
use pi_ext_subscription_router::usage::{
    BoxFuture, Scope, Status, Urls, UsageDecision, UsageService, UsageSnapshot, UsageWindow,
};
use serde_json::json;

use super::now_ms;

pub struct Env {
    pub available: HashSet<String>,
    pub decision: Option<UsageDecision>,
    pub reads: Mutex<Vec<(String, i64)>>,
}

impl Env {
    pub fn new(models: &[&str], decision: Option<UsageDecision>) -> Self {
        let available = models.iter().map(|m| m.to_string()).collect();
        Self {
            available,
            decision,
            reads: Mutex::new(Vec::new()),
        }
    }
}

impl RouteEnv for Env {
    fn available(&self, model: &ModelRef) -> bool {
        self.available.contains(&model.key())
    }

    fn refresh<'a>(
        &'a self,
        model: &'a ModelRef,
        not_before: i64,
    ) -> BoxFuture<'a, Option<UsageDecision>> {
        self.reads.lock().unwrap().push((model.key(), not_before));
        Box::pin(async move { self.decision.clone() })
    }
}

pub fn profile(config: serde_json::Value, id: &str) -> Profile {
    parse_config(&config).unwrap().profile(id).unwrap().clone()
}

pub fn two_chain(strategy: &str, extra: serde_json::Value) -> Profile {
    let mut value = json!({ "strategy": strategy, "chain": [
        { "model": "openai-codex/gpt-5.5" },
        { "model": "anthropic/claude-sonnet-5", "thinking": "high" }
    ]});
    if let (Some(target), Some(extra)) = (value.as_object_mut(), extra.as_object()) {
        target.extend(extra.clone());
    }
    profile(json!({ "profiles": { "main": value } }), "main")
}

pub fn router() -> Router {
    let urls = Urls {
        codex: "http://127.0.0.1:9".into(),
        anthropic: "http://127.0.0.1:9".into(),
    };
    Router::new(Arc::new(UsageService::new(urls, None)))
}

pub fn window(name: &str, used: f64, reset_in: i64, duration: i64) -> UsageWindow {
    UsageWindow {
        name: name.into(),
        used_percent: used,
        reset_at: Some(now_ms() + reset_in),
        duration_ms: Some(duration),
        span_source: None,
        scope: Scope::Provider,
        families: None,
    }
}

pub fn snapshot(provider: &str, windows: Vec<UsageWindow>) -> UsageSnapshot {
    UsageSnapshot {
        provider: provider.into(),
        status: Status::Available,
        windows,
        credits: None,
        tier: None,
        retrieved_at: now_ms(),
        expires_at: now_ms() + 300_000,
        message: None,
    }
}

pub fn input(reason: Reason, failed: Option<FailedRequest>, previous: Option<&str>) -> RouteInput {
    RouteInput {
        reason,
        thinking_level: "medium".into(),
        previous: previous.and_then(ModelRef::parse),
        failed,
    }
}

pub fn quota_failure(model: &str, timestamp: i64, partial: bool) -> FailedRequest {
    let content = if partial {
        json!([{ "type": "text", "text": "half" }])
    } else {
        json!([])
    };
    let message = json!({ "stopReason": "error", "errorMessage": "429 You have hit your usage limit", "content": content, "timestamp": timestamp });
    FailedRequest::from_message(ModelRef::parse(model).unwrap(), &message)
}

pub const BOTH: [&str; 2] = ["openai-codex/gpt-5.5", "anthropic/claude-sonnet-5"];
