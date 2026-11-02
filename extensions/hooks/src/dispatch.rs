use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use pi_ext_sdk::{CustomMessage, DeliverAs, Host, Snapshot};
use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::exec::{self, Context};
use crate::matcher::{Target, if_rule_matches, matches_event};
use crate::model::{HookDef, Source, canonical};
use crate::output::{Interpret, Outcome, RawRun, interpret, terminal_sequence_allowed};
use crate::runtime::Runtime;
use crate::trust::LastRun;

const NO_MATCHER: &[&str] = &[
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
    let mut runnable = Vec::new();
    for def in selected {
        if !rt.resolve_trust(host, &def).await {
            continue;
        }
        if def.source == Source::Pi
            && def.bool_field("once")
            && rt.lock().ran_once.contains(&def.hash)
        {
            continue;
        }
        runnable.push(def);
    }
    if runnable.is_empty() {
        return Vec::new();
    }
    let cx = Arc::new(Context {
        host: host.clone(),
        cwd: PathBuf::from(&snap.cwd),
        agent_dir: rt.agent_dir(host),
        session_id: snap.session_id.clone().unwrap_or_default(),
    });
    let status = runnable
        .iter()
        .find_map(|def| def.str_field("statusMessage").map(str::to_owned));
    if let Some(status) = &status {
        let _ = host.set_status("hooks", Some(status)).await;
    }
    let mut tasks = JoinSet::new();
    let mut sync_count = 0;
    for def in runnable {
        let input = hook_input(snap, &d, &def, rt);
        let sync_end = def.source.is_codex() && def.event == "SessionEnd";
        if def.is_async() && !sync_end {
            spawn_async(rt.clone(), cx.clone(), def, input);
            continue;
        }
        let cx = cx.clone();
        let index = sync_count;
        sync_count += 1;
        tasks.spawn(async move {
            let started = Instant::now();
            let raw = exec::run(&cx, &def, input).await;
            (index, def, raw, started.elapsed())
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

fn select(rt: &Runtime, d: &Dispatch, cwd: &Path) -> Vec<HookDef> {
    let mut seen = std::collections::HashSet::new();
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
        .filter(|def| !rt.is_disabled(&def.hash))
        .filter(supported)
        .filter(|def| seen.insert((def.source.is_codex(), canonical(&def.handler).to_string())))
        .collect()
}

fn supported(def: &HookDef) -> bool {
    let reason = match def.kind() {
        "command" if def.str_field("shell") == Some("powershell") && !cfg!(windows) => {
            "powershell hooks need Windows"
        }
        "command" if def.str_field("command").is_none() => "command hook without command",
        "command" | "http" => return true,
        "prompt" if !def.source.is_codex() => return true,
        "prompt" | "agent" | "mcp_tool" => "handler type is not supported",
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
    let effort = snap
        .thinking_level
        .as_deref()
        .and_then(|level| match level {
            "minimal" | "low" => Some("low"),
            "medium" | "high" | "xhigh" => Some(level),
            _ => None,
        });
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

fn spawn_async(rt: Arc<Runtime>, cx: Arc<Context>, def: HookDef, input: String) {
    tokio::spawn(async move {
        let Ok(_permit) = rt.async_slots.clone().acquire_owned().await else {
            return;
        };
        let started = Instant::now();
        let raw = exec::run(&cx, &def, input).await;
        let label = def.summary();
        let read = Interpret {
            source: def.source,
            event: &def.event,
            tool: None,
            label: &label,
            on_failure_block: false,
        };
        let mut outcome = interpret(&read, &raw);
        cap_outcome(&rt, &cx, &def, &mut outcome);
        rt.record_runs(
            &cx.host,
            vec![(
                def.hash.clone(),
                last_run(&raw, &outcome, started.elapsed()),
            )],
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

/// Codex `additionalContextLimit` and the Claude 10,000-character cap: overflow goes to a file.
fn cap_outcome(rt: &Runtime, cx: &Context, def: &HookDef, outcome: &mut Outcome) {
    let limit = if def.source.is_codex() {
        let tokens = def
            .handler
            .get("additionalContextLimit")
            .and_then(Value::as_u64)
            .unwrap_or(2500);
        if tokens == 0 {
            return;
        }
        tokens as usize * 4
    } else {
        CLAUDE_CONTEXT_LIMIT
    };
    let fields = [
        &mut outcome.context,
        &mut outcome.feedback,
        &mut outcome.block,
        &mut outcome.system_message,
        &mut outcome.initial_user_message,
    ];
    for (index, field) in fields.into_iter().enumerate() {
        let claude_field = matches!(index, 0 | 3 | 4);
        if !def.source.is_codex() && !claude_field {
            continue;
        }
        if let Some(text) = field.take() {
            *field = Some(if text.chars().count() > limit {
                spill(rt, cx, &text, def.source.is_codex(), limit)
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
