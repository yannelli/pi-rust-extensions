use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use pi_ext_sdk::{CustomMessage, DeliverAs, Host, Snapshot};
use serde_json::{Value, json};
use tokio::task::JoinSet;

use crate::exec::{self, Context};
use crate::matcher::{Target, if_rule_matches, matches};
use crate::model::{HookDef, Source};
use crate::output::{Interpret, Outcome, RawRun, interpret, terminal_sequence_allowed};
use crate::runtime::Runtime;
use crate::trust::LastRun;

const NO_MATCHER: &[&str] = &["UserPromptSubmit", "PostToolBatch", "Stop", "Interrupt"];
const TURN_EVENTS: &[&str] = &[
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
];

/// One hook event to fire. `claude` names the event for Claude and pi hooks, `codex` for Codex hooks.
pub struct Dispatch {
    pub claude: Option<&'static str>,
    pub codex: Option<&'static str>,
    pub target: Target,
    /// Claude tool name and hook `tool_input`, for `if` rules.
    pub tool: Option<(String, Value)>,
    pub payload: Value,
}

impl Dispatch {
    pub fn new(event: &'static str, target: Target, payload: Value) -> Self {
        Self {
            claude: Some(event),
            codex: Some(event),
            target,
            tool: None,
            payload,
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
    let selected = select(rt, &d);
    let mut runnable = Vec::new();
    for def in selected {
        if !rt.resolve_trust(host, &def).await {
            continue;
        }
        if def.bool_field("once") && !rt.lock().ran_once.insert(def.hash.clone()) {
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
        if def.is_async() {
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
        if def.source.is_codex() {
            outcome.context = outcome.context.map(|c| spill_context(rt, &cx, &def, c));
        }
        universal(host, &outcome).await;
        runs.push((def.hash.clone(), last_run(&raw, &outcome, elapsed)));
        outcomes.push(outcome);
    }
    rt.record_runs(host, runs);
    outcomes
}

fn select(rt: &Runtime, d: &Dispatch) -> Vec<HookDef> {
    rt.hooks()
        .into_iter()
        .filter(|def| d.event_for(def.source) == Some(def.event.as_str()))
        .filter(|def| {
            NO_MATCHER.contains(&def.event.as_str())
                || matches(def.source, def.matcher.as_deref(), &d.target)
        })
        .filter(|def| match (def.str_field("if"), &d.tool) {
            (Some(rule), Some((name, input))) => if_rule_matches(rule, name, input),
            _ => true,
        })
        .filter(|def| !rt.is_disabled(&def.hash))
        .filter(supported)
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
        if def.source.is_codex() {
            base.insert("model".into(), json!(snap.model_ref()));
            if TURN_EVENTS.contains(&def.event.as_str()) {
                base.insert("turn_id".into(), json!(format!("turn-{}", rt.lock().turn)));
            }
        }
    }
    payload.to_string()
}

fn spawn_async(rt: Arc<Runtime>, cx: Arc<Context>, def: HookDef, input: String) {
    tokio::spawn(async move {
        let Ok(_permit) = rt.async_slots.clone().acquire_owned().await else {
            return;
        };
        let started = Instant::now();
        let raw = exec::run(&cx, &def, input).await;
        let outcome = Outcome {
            error: raw.error.clone(),
            ..Outcome::default()
        };
        rt.record_runs(
            &cx.host,
            vec![(
                def.hash.clone(),
                last_run(&raw, &outcome, started.elapsed()),
            )],
        );
        if def.bool_field("asyncRewake") && raw.code == Some(2) && !raw.stderr.trim().is_empty() {
            let message = CustomMessage::new("hooks-rewake", raw.stderr.trim())
                .deliver_as(DeliverAs::FollowUp)
                .trigger_turn(true);
            if let Err(err) = cx.host.send_message(message).await {
                pi_ext_sdk::warn!("hooks: asyncRewake message failed: {err}");
            }
        }
    });
}

async fn universal(host: &Host, outcome: &Outcome) {
    if let Some(message) = &outcome.system_message {
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

/// Codex `additionalContextLimit` (tokens, about 4 chars each): overflow goes to a file.
fn spill_context(rt: &Runtime, cx: &Context, def: &HookDef, context: String) -> String {
    let limit = def
        .handler
        .get("additionalContextLimit")
        .and_then(Value::as_u64)
        .unwrap_or(2500);
    if limit == 0 || context.len() as u64 <= limit * 4 {
        return context;
    }
    let n = {
        let mut state = rt.lock();
        state.spill += 1;
        state.spill
    };
    let dir = rt.paths.temp_dir.join("hook_outputs").join(&cx.session_id);
    let path = dir.join(format!("{n}.txt"));
    if let Err(err) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &context)) {
        pi_ext_sdk::warn!("hooks: cannot write {}: {err}", path.display());
    }
    spill_message(&path, &context)
}

pub fn spill_message(path: &Path, context: &str) -> String {
    let preview: String = context.chars().take(2000).collect();
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
