//! The tool-call gate: TypeSafe reviews gated tool calls against plain-language rules.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use pi_ext_sdk::event_types::ToolCallEvent;
use pi_ext_sdk::prelude::*;
use pi_ext_sdk::{CancellationToken, NotifyType};
use serde_json::Map;

use crate::client::{Client, Purpose};
use crate::errors::{Code, TsError, TsResult};
use crate::notify;
use crate::schema::MAX_QUESTIONS;
use crate::state::{Config, Project, State};
use crate::validate::MAX_INPUT_BYTES;

pub const DEFAULT_TOOLS: [&str; 1] = ["bash"];
pub const DEFAULT_BLOCK_AT: f64 = 0.8;
pub const DEFAULT_ASK_AT: f64 = 0.5;
const CACHE_SIZE: usize = 256;
/// Room for the `model` key the client adds to each request.
const BYTE_BUDGET: usize = MAX_INPUT_BYTES - 256;

/// What a call gets when TypeSafe cannot review it. Ordered from loosest to strictest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum OnError {
    Allow,
    Ask,
    Block,
}

impl OnError {
    fn parse(text: &str) -> Option<OnError> {
        match text {
            "allow" => Some(OnError::Allow),
            "ask" => Some(OnError::Ask),
            "block" => Some(OnError::Block),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            OnError::Allow => "allow",
            OnError::Ask => "ask",
            OnError::Block => "block",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Global,
    Project,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    pub name: String,
    pub text: String,
    pub source: Source,
}

/// The `gate` object of one config file. Keys with a wrong type or range are ignored.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GateConfig {
    pub enabled: Option<bool>,
    pub tools: Option<Vec<String>>,
    pub block_at: Option<f64>,
    pub ask_at: Option<f64>,
    pub on_error: Option<OnError>,
    /// A null text removes a rule of the same name from the global file.
    pub rules: Vec<(String, Option<String>)>,
}

impl GateConfig {
    pub fn parse(value: Option<&Value>) -> GateConfig {
        let Some(gate) = value.and_then(Value::as_object) else {
            return GateConfig::default();
        };
        let threshold = |key: &str| {
            gate.get(key)
                .and_then(Value::as_f64)
                .filter(|p| (0.0..=1.0).contains(p))
        };
        let tools = gate.get("tools").and_then(Value::as_array).map(|tools| {
            let names = tools.iter().filter_map(Value::as_str);
            names.map(str::to_owned).collect()
        });
        let rule = |(name, text): (&String, &Value)| {
            let valid = (1..=100).contains(&name.chars().count());
            match text {
                Value::Null if valid => Some((name.clone(), None)),
                Value::String(text) if valid && !text.trim().is_empty() => {
                    Some((name.clone(), Some(text.trim().to_owned())))
                }
                _ => None,
            }
        };
        let rules = gate.get("rules").and_then(Value::as_object);
        GateConfig {
            enabled: gate.get("enabled").and_then(Value::as_bool),
            tools,
            block_at: threshold("blockAt"),
            ask_at: threshold("askAt"),
            on_error: gate
                .get("onError")
                .and_then(Value::as_str)
                .and_then(OnError::parse),
            rules: rules.map_or_else(Vec::new, |r| r.iter().filter_map(rule).collect()),
        }
    }
}

/// The merged gate settings of a session.
#[derive(Debug, Clone, PartialEq)]
pub struct Gate {
    pub enabled: bool,
    pub tools: Vec<String>,
    pub block_at: f64,
    pub ask_at: f64,
    pub on_error: OnError,
    pub rules: Vec<Rule>,
    /// Keys of an untrusted project file that would loosen the gate, so they do not apply.
    pub ignored: Vec<String>,
}

impl Default for Gate {
    fn default() -> Gate {
        merge(&GateConfig::default(), &GateConfig::default(), true)
    }
}

/// A trusted project wins over the global file. An untrusted project may only tighten the
/// gate: it adds rules and tools, lowers thresholds, and raises `onError`.
pub fn merge(global: &GateConfig, project: &GateConfig, trusted: bool) -> Gate {
    let mut ignored = Vec::new();
    let global_tools = global
        .tools
        .clone()
        .unwrap_or_else(|| DEFAULT_TOOLS.map(str::to_owned).to_vec());
    let tools = match (&project.tools, trusted) {
        (Some(tools), true) => tools.clone(),
        (Some(extra), false) => {
            let mut tools = global_tools;
            for tool in extra {
                if !tools.contains(tool) {
                    tools.push(tool.clone());
                }
            }
            tools
        }
        (None, _) => global_tools,
    };
    let mut threshold = |key: &str, global: Option<f64>, project: Option<f64>, default: f64| {
        let base = global.unwrap_or(default);
        match (project, trusted) {
            (Some(p), true) => p,
            (Some(p), false) if p > base => {
                ignored.push(key.to_owned());
                base
            }
            (Some(p), false) => p,
            (None, _) => base,
        }
    };
    let block_at = threshold(
        "blockAt",
        global.block_at,
        project.block_at,
        DEFAULT_BLOCK_AT,
    );
    let ask_at = threshold("askAt", global.ask_at, project.ask_at, DEFAULT_ASK_AT);
    let base_error = global.on_error.unwrap_or(OnError::Allow);
    let on_error = match (project.on_error, trusted) {
        (Some(p), true) => p,
        (Some(p), false) if p < base_error => {
            ignored.push("onError".to_owned());
            base_error
        }
        (Some(p), false) => p,
        (None, _) => base_error,
    };
    let enabled = match (project.enabled, trusted) {
        (Some(p), true) => p,
        (Some(_), false) => {
            ignored.push("enabled".to_owned());
            global.enabled.unwrap_or(false)
        }
        (None, _) => global.enabled.unwrap_or(false),
    };
    let mut rules: Vec<Rule> = global
        .rules
        .iter()
        .filter_map(|(name, text)| {
            Some(Rule {
                name: name.clone(),
                text: text.clone()?,
                source: Source::Global,
            })
        })
        .collect();
    for (name, text) in &project.rules {
        let at = rules.iter().position(|r| r.name == *name);
        match (text, at, trusted) {
            (_, Some(_), false) => ignored.push(format!("rules.{name}")),
            (None, Some(at), true) => {
                rules.remove(at);
            }
            (None, None, _) => {}
            (Some(text), at, _) => {
                let rule = Rule {
                    name: name.clone(),
                    text: text.clone(),
                    source: Source::Project,
                };
                match at {
                    Some(at) => rules[at] = rule,
                    None => rules.push(rule),
                }
            }
        }
    }
    Gate {
        enabled,
        tools,
        block_at,
        ask_at: ask_at.min(block_at),
        on_error,
        rules,
        ignored,
    }
}

impl Gate {
    pub fn from_configs(global: &Config, project: &Project) -> Gate {
        merge(
            &GateConfig::parse(global.gate.as_ref()),
            &GateConfig::parse(project.config.gate.as_ref()),
            project.trusted,
        )
    }

    pub fn reviews(&self, tool: &str) -> bool {
        self.enabled && !self.rules.is_empty() && self.tools.iter().any(|t| t == "*" || t == tool)
    }
}

/// What the gate shows TypeSafe: a bash call as its command, any other call as its input.
pub fn subject(tool: &str, input: &Value) -> Value {
    match (tool, input.get("command")) {
        ("bash", Some(Value::String(command))) => Value::String(command.clone()),
        _ => input.clone(),
    }
}

fn question(rule: &Rule) -> Value {
    let instructions = format!(
        "Does this tool call violate the rule \"Not allowed: {}\"?",
        rule.text
    );
    json!({ "type": "noul", "instructions": instructions })
}

fn clip_bytes(text: &str, keep: usize) -> String {
    let mut end = keep.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}… [truncated {} of {} bytes]",
        &text[..end],
        text.len() - end,
        text.len()
    )
}

fn request(tool: &str, subject: &Value, cwd: &str, questions: &Map<String, Value>) -> Value {
    let build = |input: &Value| json!({ "state": { "tool": tool, "input": input, "cwd": cwd }, "questions": questions });
    let mut request = build(subject);
    let mut excess = request.to_string().len().saturating_sub(BYTE_BUDGET);
    if excess == 0 {
        return request;
    }
    let text = match subject {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    let mut keep = text.len();
    while excess > 0 && keep > 0 {
        keep = keep.saturating_sub(excess);
        request = build(&Value::String(clip_bytes(&text, keep)));
        excess = request.to_string().len().saturating_sub(BYTE_BUDGET);
    }
    request
}

/// One noul question per rule, split into requests of at most `max_questions`; a large
/// input is cut so each request stays under the byte limit.
pub fn requests(
    rules: &[Rule],
    tool: &str,
    input: &Value,
    cwd: &str,
    max_questions: usize,
) -> Vec<Value> {
    let subject = subject(tool, input);
    rules
        .chunks(max_questions.clamp(1, MAX_QUESTIONS))
        .map(|chunk| {
            let questions = chunk
                .iter()
                .map(|rule| (rule.name.clone(), question(rule)))
                .collect();
            request(tool, &subject, cwd, &questions)
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub rule: String,
    pub text: String,
    pub p: f64,
}

/// The probability of a violation for each rule, highest first.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub hits: Vec<Hit>,
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    Ask,
    Block,
}

impl Verdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Allow => "allow",
            Verdict::Ask => "ask",
            Verdict::Block => "block",
        }
    }
}

fn clip(text: &str, max: usize) -> String {
    match text.chars().count() > max {
        true => format!("{}…", text.chars().take(max).collect::<String>()),
        false => text.to_owned(),
    }
}

impl Review {
    pub fn from_results(
        rules: &[Rule],
        results: &[Value],
        fallback_model: &str,
    ) -> TsResult<Review> {
        let answer = |name: &str| {
            results
                .iter()
                .find_map(|r| r["answers"][name]["noul"].as_f64())
        };
        let mut hits = rules
            .iter()
            .map(|rule| {
                Some(Hit {
                    rule: rule.name.clone(),
                    text: rule.text.clone(),
                    p: answer(&rule.name)?,
                })
            })
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| TsError::new(Code::Response, "TypeSafe left a gate rule unanswered."))?;
        hits.sort_by(|a, b| b.p.total_cmp(&a.p));
        let model = results
            .iter()
            .find_map(|r| r["model"].as_str())
            .unwrap_or(fallback_model)
            .to_owned();
        Ok(Review { hits, model })
    }

    pub fn top(&self) -> f64 {
        self.hits.first().map_or(0.0, |h| h.p)
    }

    pub fn verdict(&self, gate: &Gate) -> Verdict {
        match self.top() {
            p if p >= gate.block_at => Verdict::Block,
            p if p >= gate.ask_at => Verdict::Ask,
            _ => Verdict::Allow,
        }
    }

    /// The rules at or above `at`, for example "rule `no_prod` (…) p=0.93".
    pub fn flagged(&self, at: f64) -> String {
        let hits = self.hits.iter().filter(|h| h.p >= at);
        let parts: Vec<String> = hits
            .map(|h| format!("rule `{}` ({}) p={:.2}", h.rule, clip(&h.text, 120), h.p))
            .collect();
        parts.join("; ")
    }

    pub fn block_reason(&self, gate: &Gate) -> String {
        format!(
            "TypeSafe gate blocked this call: {} (blockAt {}, {}).",
            self.flagged(gate.block_at),
            gate.block_at,
            self.model
        )
    }

    fn ask_detail(&self, gate: &Gate) -> String {
        format!(
            "{} (askAt {}, blockAt {}, {})",
            self.flagged(gate.ask_at),
            gate.ask_at,
            gate.block_at,
            self.model
        )
    }
}

/// Sends the requests of one review concurrently; the first failure fails the review.
pub async fn review(
    client: Arc<Client>,
    rules: &[Rule],
    tool: &str,
    input: &Value,
    cwd: &str,
    cancel: &CancellationToken,
) -> TsResult<Review> {
    let mut tasks = tokio::task::JoinSet::new();
    for request in requests(rules, tool, input, cwd, client.backend.limits.max_questions) {
        let (client, cancel) = (client.clone(), cancel.clone());
        tasks.spawn(async move { client.submit(request, &cancel, Purpose::Gate).await });
    }
    let mut results = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        let result =
            joined.map_err(|_| TsError::new(Code::Response, "A TypeSafe gate request stopped."))?;
        results.push(result?);
    }
    Review::from_results(rules, &results, &client.model)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Counters {
    pub reviewed: u64,
    pub blocked: u64,
    pub asked: u64,
    pub errors: u64,
}

#[derive(Default)]
struct Cache {
    order: VecDeque<String>,
    reviews: HashMap<String, Review>,
}

#[derive(Default)]
struct Inner {
    gate: Gate,
    project: Project,
    counters: Counters,
    cache: Cache,
}

/// The session's merged gate, counters, and verdict cache.
#[derive(Default)]
pub struct GateState {
    inner: Mutex<Inner>,
}

fn cache_key(tool: &str, input: &Value, cwd: &str) -> String {
    format!("{tool}\0{cwd}\0{input}")
}

impl GateState {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("gate lock")
    }

    /// Session start: merges the files and clears the counters and cache.
    pub fn reset(&self, global: &Config, project: &Project) {
        *self.lock() = Inner {
            gate: Gate::from_configs(global, project),
            project: project.clone(),
            ..Inner::default()
        };
    }

    /// After a global file change: merges again with the session's project and clears the cache.
    pub fn reload(&self, global: &Config) {
        let mut inner = self.lock();
        inner.gate = Gate::from_configs(global, &inner.project);
        inner.cache = Cache::default();
    }

    pub fn settings(&self) -> Gate {
        self.lock().gate.clone()
    }

    pub fn counters(&self) -> Counters {
        self.lock().counters
    }

    fn count(&self, update: impl FnOnce(&mut Counters)) {
        update(&mut self.lock().counters);
    }

    fn cached(&self, key: &str) -> Option<Review> {
        self.lock().cache.reviews.get(key).cloned()
    }

    fn remember(&self, key: String, review: Review) {
        let mut inner = self.lock();
        let cache = &mut inner.cache;
        if cache.reviews.insert(key.clone(), review).is_none() {
            cache.order.push_back(key);
        }
        while cache.order.len() > CACHE_SIZE {
            if let Some(old) = cache.order.pop_front() {
                cache.reviews.remove(&old);
            }
        }
    }
}

/// Reviews one call against the gate's rules, without the cache.
pub async fn check(
    state: &State,
    gate: &Gate,
    tool: &str,
    input: &Value,
    cwd: &str,
    cancel: &CancellationToken,
) -> TsResult<Review> {
    if let Some(error) = &state.backend_error {
        return Err(TsError::configuration(error.clone()));
    }
    review(state.get_client()?, &gate.rules, tool, input, cwd, cancel).await
}

fn summary(tool: &str, input: &Value) -> String {
    let shown = match subject(tool, input) {
        Value::String(text) => text,
        other => other.to_string(),
    };
    format!("{tool}: {}", clip(&shown, 300))
}

pub async fn tool_call(state: Arc<State>, ev: EventCtx) -> Result<EventReply> {
    let event: ToolCallEvent = ev.event_as()?;
    let gate = state.gate.settings();
    if !gate.reviews(&event.tool_name) || !state.enabled() {
        return Ok(EventReply::none());
    }
    let (host, cwd) = (ev.host(), ev.snapshot().cwd.as_str());
    let (tool, input) = (event.tool_name.as_str(), &event.input);
    let key = cache_key(tool, input, cwd);
    state.gate.count(|c| c.reviewed += 1);
    let review = match state.gate.cached(&key) {
        Some(review) => review,
        None => match check(&state, &gate, tool, input, cwd, ev.cancel_token()).await {
            Ok(review) => {
                state.gate.remember(key, review.clone());
                review
            }
            Err(error) => return on_error(&state, host, &gate, tool, input, &error).await,
        },
    };
    match review.verdict(&gate) {
        Verdict::Allow => Ok(EventReply::none()),
        Verdict::Block => {
            state.gate.count(|c| c.blocked += 1);
            Ok(EventReply::block(review.block_reason(&gate)))
        }
        Verdict::Ask => {
            state.gate.count(|c| c.asked += 1);
            let detail = review.ask_detail(&gate);
            if !host.has_ui() {
                state.gate.count(|c| c.blocked += 1);
                return Ok(EventReply::block(format!(
                    "TypeSafe gate flagged this call and there is no UI to ask the user: {detail}."
                )));
            }
            let message = format!("{}\nFlagged: {detail}.", summary(tool, input));
            match host
                .confirm("TypeSafe gate: allow this call?", &message)
                .await
            {
                Ok(true) => Ok(EventReply::none()),
                _ => {
                    state.gate.count(|c| c.blocked += 1);
                    Ok(EventReply::block(format!(
                        "The user denied this tool call after the TypeSafe gate flagged it: {detail}."
                    )))
                }
            }
        }
    }
}

async fn on_error(
    state: &State,
    host: &Host,
    gate: &Gate,
    tool: &str,
    input: &Value,
    error: &TsError,
) -> Result<EventReply> {
    state.gate.count(|c| c.errors += 1);
    let cause = &error.message;
    match gate.on_error {
        OnError::Allow => {
            let text = format!(
                "TypeSafe gate could not review a tool call and allowed it (onError allow): {cause}"
            );
            let key = format!("gate:{}", error.code.as_str());
            state.call_out(host, &key, &text).await;
            Ok(EventReply::none())
        }
        OnError::Ask if host.has_ui() => {
            let message = format!(
                "{}\nTypeSafe could not review it: {cause}",
                summary(tool, input)
            );
            match host
                .confirm("TypeSafe gate: allow this unreviewed call?", &message)
                .await
            {
                Ok(true) => Ok(EventReply::none()),
                _ => {
                    state.gate.count(|c| c.blocked += 1);
                    Ok(EventReply::block(format!(
                        "The user denied this tool call after the TypeSafe gate could not review it: {cause}"
                    )))
                }
            }
        }
        on_error => {
            state.gate.count(|c| c.blocked += 1);
            let why = match on_error {
                OnError::Ask => "onError is ask and there is no UI to ask the user",
                _ => "onError is block",
            };
            Ok(EventReply::block(format!(
                "TypeSafe gate could not review this call ({cause}) and {why}."
            )))
        }
    }
}

fn source(source: Source) -> &'static str {
    match source {
        Source::Global => "global",
        Source::Project => "project",
    }
}

pub fn status_text(state: &State) -> String {
    let gate = state.gate.settings();
    let mut lines = vec![format!(
        "TypeSafe gate: {}. Reviews {}. Blocks at p >= {}, asks at p >= {}, on error {}.",
        if gate.enabled { "on" } else { "off" },
        match gate.tools.is_empty() {
            true => "no tools".to_owned(),
            false => gate.tools.join(", "),
        },
        gate.block_at,
        gate.ask_at,
        gate.on_error.as_str()
    )];
    if gate.rules.is_empty() {
        lines.push("Rules: none, so no call is reviewed. Add them under gate.rules.".to_owned());
    } else {
        lines.push("Rules:".to_owned());
        for rule in &gate.rules {
            lines.push(format!(
                "- {} ({}): {}",
                rule.name,
                source(rule.source),
                rule.text
            ));
        }
    }
    let c = state.gate.counters();
    let spend = state
        .client()
        .map(|client| client.spend().gate)
        .unwrap_or_default();
    lines.push(format!(
        "Session: {} reviewed, {} blocked, {} asked, {} errors; {} requests, {} input tokens (~${:.4}).",
        c.reviewed, c.blocked, c.asked, c.errors, spend.requests_started, spend.input_tokens, spend.usd
    ));
    if !gate.ignored.is_empty() {
        lines.push(format!(
            "Ignored project keys (project not trusted): {}.",
            gate.ignored.join(", ")
        ));
    }
    if gate.enabled && !state.enabled() {
        lines.push(
            "Inactive: TypeSafe is disabled. Run /typesafe enable to start reviews.".to_owned(),
        );
    }
    lines.join("\n")
}

async fn test(state: &State, host: &Host, command: &str) -> TsResult<String> {
    let gate = state.gate.settings();
    if command.is_empty() {
        return Ok("Usage: /typesafe gate test <command>.".to_owned());
    }
    if !state.enabled() {
        return Ok("TypeSafe is disabled. Run /typesafe enable first.".to_owned());
    }
    if gate.rules.is_empty() {
        return Ok("The gate has no rules. Add them under gate.rules.".to_owned());
    }
    let input = json!({ "command": command });
    let cwd = host.cwd().to_string_lossy().into_owned();
    let review = check(
        state,
        &gate,
        "bash",
        &input,
        &cwd,
        &CancellationToken::new(),
    )
    .await?;
    let scores: Vec<String> = review
        .hits
        .iter()
        .map(|h| format!("{} p={:.2}", h.rule, h.p))
        .collect();
    Ok(format!(
        "TypeSafe gate test: {}. {} ({}).",
        review.verdict(&gate).as_str(),
        scores.join(", "),
        review.model
    ))
}

/// `/typesafe gate [on|off|test <command>]`.
pub async fn command(state: &State, host: &Host, args: &str) -> Result<()> {
    let (verb, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
    let (text, level) = match verb {
        "" => (status_text(state), NotifyType::Info),
        "on" | "off" => {
            let enabled = verb == "on";
            let path = Config::save_gate_enabled(&state.agent_dir, enabled)?;
            state.gate.reload(&Config::load(&state.agent_dir));
            let gate = state.gate.settings();
            let mut text = format!(
                "Saved gate.enabled {enabled} to {}. The gate is {}.",
                path.display(),
                if gate.enabled { "on" } else { "off" }
            );
            if gate.enabled != enabled {
                text.push_str(" The project file sets gate.enabled and wins.");
            } else if enabled && !state.enabled() {
                text.push_str(
                    " It reviews calls only while TypeSafe is enabled: run /typesafe enable.",
                );
            }
            (text, NotifyType::Info)
        }
        "test" => match test(state, host, rest.trim()).await {
            Ok(text) => (text, NotifyType::Info),
            Err(error) => {
                let on_error = state.gate.settings().on_error.as_str();
                let text = format!(
                    "TypeSafe gate test failed: {} With onError {on_error}, a gated call gets {on_error}.",
                    error.message
                );
                (text, NotifyType::Warning)
            }
        },
        _ => (
            "Usage: /typesafe gate [on | off | test <command>].".to_owned(),
            NotifyType::Warning,
        ),
    };
    notify(host, &text, level).await;
    Ok(())
}

#[cfg(test)]
#[path = "gate_tests.rs"]
mod tests;
