use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use pi_ext_sdk::{CustomMessage, DeliverAs, Host, Snapshot};
use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::exec::http::HttpPolicy;
use crate::exec::{self, Context};
use crate::limit::{self, Gate};
use crate::matcher::{Target, if_rule_matches, matches_event};
use crate::model::{HookDef, Layer, Source};
use crate::output::{Interpret, Outcome, RawRun, interpret, terminal_sequence_allowed};
use crate::policy;
use crate::runtime::Runtime;
use crate::trust::LastRun;

pub const NO_MATCHER: &[&str] = &[
    "TaskCreated",
    "TaskCompleted",
    "TeammateIdle",
    "CwdChanged",
    "MessageDisplay",
    "UserPromptSubmit",
    "PostToolBatch",
    "Stop",
    "Interrupt",
    "PreModelRequest",
    "PostModelRequest",
];
const TURN_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "PreCompact",
    "PostCompact",
    "SubagentStart",
    "SubagentStop",
    "Interrupt",
    "PreModelRequest",
    "PostModelRequest",
];
const TOOL_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "PostToolUseFailure",
    "PermissionRequest",
    "PermissionDenied",
];
/// Claude caps each context string and plain stdout at 10,000 characters.
const CLAUDE_CONTEXT_LIMIT: usize = 10_000;

/// One hook event to fire. `claude` names the event for Claude and pi hooks, `codex` for Codex hooks.
pub struct Dispatch {
    pub claude: Option<&'static str>,
    pub codex: Option<&'static str>,
    pub target: Target,
    /// Claude tool name and hook `tool_input`, for `if` rules.
    pub tool: Option<(String, Value)>,
    pub payload: Value,
    /// Fields that replace `payload` fields for Codex hooks.
    pub codex_payload: Option<Value>,
}

impl Dispatch {
    pub fn new(event: &'static str, target: Target, payload: Value) -> Self {
        Self {
            claude: Some(event),
            codex: Some(event),
            target,
            tool: None,
            payload,
            codex_payload: None,
        }
    }

    pub fn claude_only(mut self) -> Self {
        self.codex = None;
        self
    }

    fn event_for(&self, source: Source) -> Option<&'static str> {
        if source.is_codex() {
            self.codex
        } else {
            self.claude
        }
    }
}

/// Common stdin fields.
pub fn base_payload(snap: &Snapshot) -> Value {
    json!({
        "session_id": snap.session_id.clone().unwrap_or_default(),
        "transcript_path": snap.session_file,
        "cwd": snap.cwd,
        "permission_mode": "default",
    })
}

/// Runs the matching, trusted hooks in parallel and returns their outcomes in config order.
pub async fn dispatch(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    d: Dispatch,
) -> Vec<Outcome> {
    rt.ensure_loaded(host, snap).await;
    let selected = select(rt, &d, Path::new(&snap.cwd));
    let runnable = policy::runnable(rt, host, selected, rt.conflict_mode()).await;
    if runnable.is_empty() {
        return Vec::new();
    }
    let cx = Arc::new(Context {
        host: host.clone(),
        cwd: PathBuf::from(&snap.cwd),
        agent_dir: rt.agent_dir(host),
        session_id: snap.session_id.clone().unwrap_or_default(),
        http: rt.http_policy(),
        effort: claude_effort(snap),
    });
    let status = runnable
        .iter()
        .find_map(|def| def.str_field("statusMessage").map(str::to_owned));
    if let Some(status) = &status {
        let _ = host.set_status("hooks", Some(status)).await;
    }
    let gate = Gate::new(
        rt.lock().loaded.as_ref().map(|l| &l.options),
        &rt.hook_slots,
    );
    let mut tasks = JoinSet::new();
    let mut sync_count = 0;
    for def in runnable {
        let input = hook_input(snap, &d, &def, rt);
        let sync_end = def.source.is_codex() && def.event == "SessionEnd";
        if def.is_async() && !sync_end {
            spawn_async(rt.clone(), cx.clone(), def, input);
            continue;
        }
        let (cx, gate) = (cx.clone(), gate.clone());
        let index = sync_count;
        sync_count += 1;
        tasks.spawn(async move {
            let (raw, elapsed) = limit::run(gate.as_ref(), &def, timed(&cx, &def, input)).await;
            (index, def, raw, elapsed)
        });
    }
    let mut done = tasks.join_all().await;
    done.sort_by_key(|(index, ..)| *index);
    if status.is_some() {
        let _ = host.set_status("hooks", None).await;
    }
    let tool = d.tool.as_ref().map(|(name, _)| name.as_str());
    let mut outcomes = Vec::new();
    let mut runs = Vec::new();
    for (_, def, raw, elapsed) in done {
        let label = def.summary();
        let read = Interpret {
            source: def.source,
            event: &def.event,
            tool,
            label: &label,
            on_failure_block: def.str_field("onFailure") == Some("block"),
            kind: def.kind(),
            plugin: def.plugin.is_some(),
        };
        let mut outcome = interpret(&read, &raw);
        if let Some(error) = &outcome.error {
            pi_ext_sdk::warn!("hooks: {def}: {error}");
        }
        if def.bool_field("once") && raw.code == Some(0) && outcome.block.is_none() {
            rt.lock().ran_once.insert(def.hash.clone());
        }
        cap_outcome(rt, &cx, &def, &mut outcome);
        universal(host, &def.event, &outcome).await;
        runs.push((def.hash.clone(), last_run(&raw, &outcome, elapsed)));
        outcomes.push(outcome);
    }
    rt.record_runs(host, runs);
    outcomes
}

/// Matched, enabled, and supported hooks in config order; [`policy::runnable`] picks the ones to run.
fn select(rt: &Runtime, d: &Dispatch, cwd: &Path) -> Vec<HookDef> {
    let http = rt.http_policy();
    rt.hooks()
        .into_iter()
        .filter(|def| d.event_for(def.source) == Some(def.event.as_str()))
        .filter(|def| {
            NO_MATCHER.contains(&def.event.as_str())
                || matches_event(def.source, &def.event, def.matcher.as_deref(), &d.target)
        })
        .filter(|def| match (def.str_field("if"), &d.tool) {
            (Some(_), _) if !TOOL_EVENTS.contains(&def.event.as_str()) => false,
            (Some(rule), Some((name, input))) => if_rule_matches(rule, name, input, cwd),
            _ => true,
        })
        .filter(|def| def.layer == Layer::Managed || !rt.is_disabled(&def.hash))
        .filter(|def| !(def.kind() == "mcp_tool" && before_mcp(&def.event, &d.payload)))
        .filter(supported)
        .filter(|def| url_allowed(&http, def))
        .collect()
}

/// Claude `allowedHttpHookUrls`: an http hook runs only when its URL matches the merged list.
fn url_allowed(http: &HttpPolicy, def: &HookDef) -> bool {
    if def.kind() != "http" || def.source.is_codex() {
        return true;
    }
    let url = def.str_field("url").unwrap_or_default();
    let allowed = http.url_allowed(url);
    if !allowed {
        pi_ext_sdk::info!("hooks: skipping {def}: URL is not in allowedHttpHookUrls");
    }
    allowed
}

/// Setup and SessionStart at launch run before MCP servers are available, so `mcp_tool` skips them.
fn before_mcp(event: &str, payload: &Value) -> bool {
    let source = payload.get("source").and_then(Value::as_str);
    event == "Setup" || (event == "SessionStart" && matches!(source, Some("startup" | "resume")))
}

fn supported(def: &HookDef) -> bool {
    let reason = match def.kind() {
        "command" if def.str_field("shell") == Some("powershell") && !cfg!(windows) => {
            "powershell hooks need Windows"
        }
        "command" if def.str_field("command").is_none() => "command hook without command",
        "command"
            if def.plugin.is_some()
                && def.handler.get("args").is_none()
                && def
                    .str_field("command")
                    .is_some_and(|c| c.contains("${user_config.")) =>
        {
            "shell-form plugin hooks cannot use ${user_config.*}; read $CLAUDE_PLUGIN_OPTION_<KEY> or set args"
        }
        "mcp_tool" if def.source.is_codex() && def.event == "SessionEnd" => {
            "Codex SessionEnd does not support mcp_tool hooks"
        }
        "command" | "http" | "mcp_tool" => return true,
        "agent" if def.event == "PermissionRequest" => {
            "agent hooks do not run on PermissionRequest"
        }
        "prompt" | "agent" if !def.source.is_codex() => return true,
        "prompt" | "agent" => "Codex skips prompt and agent handlers",
        _ => "unknown handler type",
    };
    pi_ext_sdk::info!("hooks: skipping {def}: {reason}");
    false
}

fn hook_input(snap: &Snapshot, d: &Dispatch, def: &HookDef, rt: &Runtime) -> String {
    let mut payload = base_payload(snap);
    if let (Value::Object(base), Value::Object(extra)) = (&mut payload, &d.payload) {
        base.extend(extra.clone());
        base.insert("hook_event_name".into(), json!(def.event));
        if !def.source.is_codex() {
            claude_common(base, snap, def, rt);
        }
        if def.source.is_codex() {
            if let Some(Value::Object(codex)) = &d.codex_payload {
                base.extend(codex.clone());
            }
            let model = snap.model.as_ref().map(|m| m.id.clone());
            base.insert("model".into(), json!(model));
            if TURN_EVENTS.contains(&def.event.as_str()) {
                base.insert("turn_id".into(), json!(format!("turn-{}", rt.lock().turn)));
            }
        }
    }
    payload.to_string()
}

/// Claude common fields: `prompt_id`, `effort`, `session_title`, and `mcp_server` for MCP tools.
fn claude_common(
    base: &mut serde_json::Map<String, Value>,
    snap: &Snapshot,
    def: &HookDef,
    rt: &Runtime,
) {
    let state = rt.lock();
    if let Some(id) = &state.prompt_id {
        base.insert("prompt_id".into(), json!(id));
    }
    let effort = claude_effort(snap);
    let tool_context =
        TOOL_EVENTS.contains(&def.event.as_str()) || matches!(&*def.event, "Stop" | "SubagentStop");
    if let Some(level) = effort.filter(|_| tool_context) {
        base.insert("effort".into(), json!({ "level": level }));
    }
    if matches!(&*def.event, "SessionStart" | "UserPromptSubmit")
        && let Some(title) = &snap.session_name
    {
        base.insert("session_title".into(), json!(title));
    }
    let server = base
        .get("tool_name")
        .and_then(Value::as_str)
        .and_then(|name| name.strip_prefix("mcp__"))
        .and_then(|rest| rest.split_once("__"))
        .map(|(server, _)| server.to_owned());
    if let Some(server) = server.filter(|_| TOOL_EVENTS.contains(&def.event.as_str())) {
        let source = state
            .mcp_sources
            .as_ref()
            .and_then(|m| m.get(&server).cloned())
            .unwrap_or_else(|| "user".into());
        base.insert(
            "mcp_server".into(),
            json!({ "name": server, "source": source }),
        );
    }
}

/// Claude async hooks may print their JSON on a line of its own among other stdout.
fn async_json_line(stdout: &str) -> String {
    if crate::output::json_output(stdout).is_some() {
        return stdout.to_owned();
    }
    let line = stdout
        .lines()
        .rfind(|line| crate::output::json_output(line).is_some());
    line.unwrap_or(stdout).to_owned()
}

/// The pi thinking level as a Claude effort level (`minimal` and `low` are `low`).
fn claude_effort(snap: &Snapshot) -> Option<&'static str> {
    match snap.thinking_level.as_deref()? {
        "minimal" | "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        "xhigh" => Some("xhigh"),
        _ => None,
    }
}

/// Runs one hook and measures how long it ran.
async fn timed(cx: &Context, def: &HookDef, input: String) -> (RawRun, std::time::Duration) {
    let started = Instant::now();
    (exec::run(cx, def, input).await, started.elapsed())
}

fn spawn_async(rt: Arc<Runtime>, cx: Arc<Context>, def: HookDef, input: String) {
    tokio::spawn(async move {
        let Ok(_permit) = rt.async_slots.clone().acquire_owned().await else {
            return;
        };
        let (mut raw, elapsed) = timed(&cx, &def, input).await;
        if !def.source.is_codex() {
            raw.stdout = async_json_line(&raw.stdout);
        }
        let label = def.summary();
        let read = Interpret {
            source: def.source,
            event: &def.event,
            tool: None,
            label: &label,
            on_failure_block: false,
            kind: def.kind(),
            plugin: def.plugin.is_some(),
        };
        let mut outcome = interpret(&read, &raw);
        cap_outcome(&rt, &cx, &def, &mut outcome);
        rt.record_runs(
            &cx.host,
            vec![(def.hash.clone(), last_run(&raw, &outcome, elapsed))],
        );
        if let Some(message) = &outcome.system_message {
            let _ = cx.host.notify_warning(message.clone()).await;
        }
        if let Some(context) = outcome.context.filter(|c| !c.trim().is_empty()) {
            rt.lock().held_context.push(context);
        }
        let rewake = [raw.stderr.trim(), raw.stdout.trim()]
            .into_iter()
            .find(|t| !t.is_empty());
        if def.bool_field("asyncRewake")
            && raw.code == Some(2)
            && let Some(text) = rewake
        {
            let message = CustomMessage::new("hooks-rewake", text)
                .deliver_as(DeliverAs::FollowUp)
                .trigger_turn(true);
            if let Err(err) = cx.host.send_message(message).await {
                pi_ext_sdk::warn!("hooks: asyncRewake message failed: {err}");
            }
        }
    });
}

/// Codex `additionalContextLimit` (tokens) applies to `additionalContext` on these events only.
const CODEX_CONTEXT_EVENTS: &[&str] = &[
    "PreToolUse",
    "PostToolUse",
    "SessionStart",
    "UserPromptSubmit",
    "SubagentStart",
];
const CODEX_DEFAULT_TOKENS: usize = 2500;

/// Characters of Codex context a handler may return; `None` when `additionalContextLimit` is 0.
fn codex_context_limit(def: &HookDef) -> Option<usize> {
    let default = Some(CODEX_DEFAULT_TOKENS * 4);
    match def
        .handler
        .get("additionalContextLimit")
        .and_then(Value::as_u64)
    {
        Some(_) if !CODEX_CONTEXT_EVENTS.contains(&def.event.as_str()) => {
            pi_ext_sdk::info!("hooks: additionalContextLimit is ignored on {}", def.event);
            default
        }
        Some(0) => None,
        Some(tokens) => Some(tokens as usize * 4),
        None => default,
    }
}

/// The Codex context limit and the Claude 10,000-character cap: overflow goes to a file.
fn cap_outcome(rt: &Runtime, cx: &Context, def: &HookDef, outcome: &mut Outcome) {
    let codex = def.source.is_codex();
    let (context, other) = if codex {
        (codex_context_limit(def), Some(CODEX_DEFAULT_TOKENS * 4))
    } else {
        (Some(CLAUDE_CONTEXT_LIMIT), Some(CLAUDE_CONTEXT_LIMIT))
    };
    let fields = [
        (&mut outcome.context, context),
        (&mut outcome.feedback, other.filter(|_| codex)),
        (&mut outcome.block, other.filter(|_| codex)),
        (&mut outcome.system_message, other),
        (&mut outcome.initial_user_message, other.filter(|_| !codex)),
    ];
    for (field, limit) in fields {
        let Some(limit) = limit else {
            continue;
        };
        if let Some(text) = field.take() {
            *field = Some(if text.chars().count() > limit {
                spill(rt, cx, &text, codex, limit)
            } else {
                text
            });
        }
    }
}

/// Claude discards `systemMessage` on these events (DirectoryAdded gives it to the model).
const NO_SYSTEM_MESSAGE: &[&str] = &[
    "Setup",
    "InstructionsLoaded",
    "Notification",
    "DirectoryAdded",
    "MessageDisplay",
    "ConfigChange",
    "WorktreeCreate",
    "WorktreeRemove",
    "Elicitation",
    "ElicitationResult",
    "StopFailure",
];

async fn universal(host: &Host, event: &str, outcome: &Outcome) {
    if let Some(message) = outcome
        .system_message
        .as_ref()
        .filter(|_| !NO_SYSTEM_MESSAGE.contains(&event))
    {
        let _ = host.notify_warning(message.clone()).await;
    }
    if let Some(seq) = &outcome.terminal_sequence {
        if terminal_sequence_allowed(seq) {
            let _ = host.write_terminal(seq, false).await;
        } else {
            pi_ext_sdk::warn!("hooks: terminalSequence {seq:?} is not allowed");
        }
    }
}

fn spill(rt: &Runtime, cx: &Context, text: &str, head_and_tail: bool, limit: usize) -> String {
    let n = {
        let mut state = rt.lock();
        state.spill += 1;
        state.spill
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let id = crate::model::trust_hash(
        &cx.session_id,
        &n.to_string(),
        Some(&nanos.to_string()),
        &json!(text.len()),
    );
    let dir = rt.paths.temp_dir.join("hook_outputs").join(&cx.session_id);
    let path = dir.join(format!("{}.txt", &id[..32]));
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text)) {
        pi_ext_sdk::warn!("hooks: cannot write {}: {err}", path.display());
    }
    spill_message(&path, text, head_and_tail, limit)
}

/// Claude previews the first 2,000 characters; Codex previews head and tail within the limit.
pub fn spill_message(path: &Path, text: &str, head_and_tail: bool, limit: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let preview = if head_and_tail {
        let half = (limit / 2).min(chars.len() / 2);
        let head: String = chars[..half].iter().collect();
        let tail: String = chars[chars.len() - half..].iter().collect();
        format!("{head}\n[...]\n{tail}")
    } else {
        chars.iter().take(2000).collect()
    };
    format!(
        "Hook output is too large; full text in {}. Preview:\n{preview}",
        path.display()
    )
}

fn last_run(raw: &RawRun, outcome: &Outcome, elapsed: std::time::Duration) -> LastRun {
    let cut = |text: &str| text.chars().take(4000).collect::<String>();
    let summary = outcome.summary();
    LastRun {
        at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
        duration_ms: elapsed.as_millis() as u64,
        exit: raw.code,
        result: if summary.is_empty() {
            "ok".into()
        } else {
            summary
        },
        stdout: cut(&raw.stdout),
        stderr: cut(&raw.stderr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Scope;

    #[test]
    fn async_json_can_share_stdout_with_other_lines() {
        let out = "loading profile\n{\"systemMessage\":\"done\"}\nbye\n";
        assert_eq!(async_json_line(out), "{\"systemMessage\":\"done\"}");
        assert_eq!(async_json_line("{\n  \"a\": 1\n}"), "{\n  \"a\": 1\n}");
        assert_eq!(async_json_line("plain"), "plain");
    }

    #[test]
    fn codex_context_limit_applies_to_context_events_only() {
        let def = |event: &str, limit: u64| {
            let handler = json!({ "command": "x", "additionalContextLimit": limit });
            HookDef::new(
                Source::Codex,
                Scope::Global,
                "/f".into(),
                event,
                None,
                handler,
            )
        };
        assert_eq!(codex_context_limit(&def("SessionStart", 100)), Some(400));
        assert_eq!(codex_context_limit(&def("SessionStart", 0)), None);
        assert_eq!(codex_context_limit(&def("Stop", 100)), Some(10_000));
    }
}
