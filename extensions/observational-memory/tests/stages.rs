mod fixtures;

use std::collections::VecDeque;
use std::sync::Mutex;

use fixtures::*;
use pi_ext_observational_memory::agents::completion::{Completer, extract_json_object};
use pi_ext_observational_memory::agents::dropper::select_drop_candidates;
use pi_ext_observational_memory::config::{Config, merge};
use pi_ext_observational_memory::debug_log::DebugLog;
use pi_ext_observational_memory::ledger::*;
use pi_ext_observational_memory::pipeline::{PipelineCtx, run_pipeline};
use pi_ext_observational_memory::runtime::{MemoryHost, Runtime};
use serde_json::{Value, json};

struct Scripted {
    replies: Mutex<VecDeque<Result<String, String>>>,
    prompts: Mutex<Vec<(String, String)>>,
}

impl Scripted {
    fn new(replies: &[Result<&str, &str>]) -> Self {
        let replies = replies
            .iter()
            .map(|r| r.map(str::to_owned).map_err(str::to_owned))
            .collect();
        Self {
            replies: Mutex::new(replies),
            prompts: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> usize {
        self.prompts.lock().unwrap().len()
    }
}

impl Completer for Scripted {
    async fn complete(&self, system: &str, prompt: &str) -> Result<String, String> {
        self.prompts
            .lock()
            .unwrap()
            .push((system.to_owned(), prompt.to_owned()));
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Err("no scripted reply".into()))
    }
}

#[derive(Default)]
struct FakeHost {
    entries: Mutex<Vec<Value>>,
    notices: Mutex<Vec<(String, String)>>,
}

impl FakeHost {
    fn with(entries: Vec<Value>) -> Self {
        Self {
            entries: Mutex::new(entries),
            ..Self::default()
        }
    }

    fn appended(&self, custom_type: &str) -> Vec<Value> {
        let entries = self.entries.lock().unwrap();
        entries
            .iter()
            .filter(|e| e["customType"] == custom_type)
            .map(|e| e["data"].clone())
            .collect()
    }
}

impl MemoryHost for FakeHost {
    async fn branch(&self) -> Result<Vec<Value>, String> {
        Ok(self.entries.lock().unwrap().clone())
    }

    async fn append(&self, custom_type: &str, data: Value) -> Result<(), String> {
        let mut entries = self.entries.lock().unwrap();
        let id = format!("m{}", entries.len());
        entries.push(custom(&id, custom_type, data));
        Ok(())
    }

    async fn notify(&self, message: String, level: &str) {
        self.notices
            .lock()
            .unwrap()
            .push((level.to_owned(), message));
    }

    async fn context_tokens(&self) -> Option<u64> {
        None
    }
}

async fn run(host: &FakeHost, completer: &Scripted, config: &Config, runtime: &Runtime) {
    let log = DebugLog::default();
    let ctx = PipelineCtx {
        config,
        runtime,
        log: &log,
        has_ui: true,
        session_identity: Some("s1".into()),
        memory_context_window: None,
    };
    run_pipeline(host, completer, &ctx).await;
}

fn observer_reply(sources: &[&str]) -> String {
    json!({ "observations": [
        { "timestamp": "2026-10-29 10:00", "content": "User asked for a parser.", "relevance": "medium", "sourceEntryIds": sources },
        { "timestamp": "bad", "content": "rejected by schema", "relevance": "low", "sourceEntryIds": ["u0"] },
    ] })
    .to_string()
}

#[tokio::test]
async fn observer_records_fenced_json_and_skips_reflector_below_threshold() {
    let host = FakeHost::with(conversation(3, 16_000));
    let reply = format!("```json\n{}\n```", observer_reply(&["u1", "u0", "nope"]));
    let ok = observer_reply(&["u1", "u0"]);
    let completer = Scripted::new(&[Ok(&reply), Ok(&ok)]);
    let (config, runtime) = (
        merge(&[json!({ "reflectAfterTokens": 100_000 })], None),
        Runtime::default(),
    );
    run(&host, &completer, &config, &runtime).await;

    assert_eq!(completer.calls(), 1);
    assert!(
        host.appended(OM_OBSERVATIONS_RECORDED).is_empty(),
        "unknown source id rejects the item"
    );
    assert_eq!(
        runtime
            .state()
            .observer_empty_backoff
            .as_ref()
            .unwrap()
            .tokens_at_empty,
        12_000
    );

    let completer = Scripted::new(&[Ok(&ok)]);
    let runtime = Runtime::default();
    run(&host, &completer, &config, &runtime).await;
    let recorded = host.appended(OM_OBSERVATIONS_RECORDED);
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0]["coversUpToId"], "u2");
    let obs = &recorded[0]["observations"][0];
    assert_eq!(obs["sourceEntryIds"], json!(["u0", "u1"]));
    assert_eq!(obs["id"].as_str().unwrap().len(), 12);
    assert!(
        completer.prompts.lock().unwrap()[0]
            .1
            .contains("[Source entry id: u2]")
    );
}

#[tokio::test]
async fn bad_json_retries_once_then_aborts_with_warning() {
    let host = FakeHost::with(conversation(3, 16_000));
    let completer = Scripted::new(&[Ok("not json"), Ok("{\"observations\": 3}")]);
    let runtime = Runtime::default();
    run(&host, &completer, &merge(&[], None), &runtime).await;
    assert_eq!(completer.calls(), 2);
    assert!(
        completer.prompts.lock().unwrap()[1]
            .1
            .contains("Your previous reply could not be used")
    );
    assert!(
        host.entries
            .lock()
            .unwrap()
            .iter()
            .all(|e| e["type"] == "message")
    );
    let warning = host
        .notices
        .lock()
        .unwrap()
        .iter()
        .find(|(level, _)| level == "warning")
        .cloned()
        .unwrap();
    assert!(
        warning
            .1
            .starts_with("Observational memory: observer failed: invalid JSON reply")
    );
    assert!(runtime.state().last_observer_error.is_some());

    let model_error = Scripted::new(&[Err("No credentials for openai")]);
    let runtime = Runtime::default();
    run(&host, &model_error, &merge(&[], None), &runtime).await;
    assert_eq!(
        runtime.state().last_observer_error.as_deref(),
        Some("No credentials for openai")
    );
}

#[tokio::test]
async fn full_pipeline_observes_reflects_and_drops() {
    let olds: Vec<_> = (0..40)
        .map(|i| {
            observation(
                &format!("old routine fact {i} {}", "w".repeat(60)),
                "low",
                &["u0"],
            )
        })
        .collect();
    let mut entries = conversation(3, 30_000);
    entries.insert(1, observations_entry("m-old", &olds, "u0"));
    let host = FakeHost::with(entries);
    let reflect = json!({ "reflections": [{ "content": "Routine facts are covered.", "supportingObservationIds": [olds[0].id, olds[1].id] }] }).to_string();
    let drop = json!({ "ids": [olds[1].id, olds[0].id, "zzzzzzzzzzzz", olds[2].id] }).to_string();
    let observe = observer_reply(&["u1"]);
    let completer = Scripted::new(&[Ok(&observe), Ok(&reflect), Ok(&drop)]);
    let config = merge(&[json!({ "observationsPoolMaxTokens": 1000 })], None);
    run(&host, &completer, &config, &Runtime::default()).await;

    assert_eq!(completer.calls(), 3);
    let reflections = host.appended(OM_REFLECTIONS_RECORDED);
    assert_eq!(reflections[0]["coversUpToId"], "u2");
    assert_eq!(
        reflections[0]["reflections"][0]["supportingObservationIds"],
        json!([olds[0].id, olds[1].id])
    );
    let dropped = host.appended(OM_OBSERVATIONS_DROPPED);
    assert_eq!(
        dropped[0]["observationIds"],
        json!([olds[1].id, olds[0].id, olds[2].id])
    );
    assert_eq!(dropped[0]["coversUpToId"], "u2");
    assert_eq!(
        host.appended(OM_OBSERVATIONS_RECORDED)[1]["coversUpToId"],
        "u2"
    );
}

#[test]
fn drop_candidates_order_by_coverage_relevance_then_age() {
    let mut a = observation("a", "critical", &["u"]);
    let b = observation("b", "low", &["u"]);
    let c = observation("c", "low", &["u"]);
    a.timestamp = "2026-10-01 09:00".into();
    let r = reflection("covers a", &[&a.id]);
    let obs = vec![a.clone(), b.clone(), c.clone()];
    let ids = vec![c.id.clone(), a.id.clone(), b.id.clone(), c.id.clone()];
    assert_eq!(
        select_drop_candidates(&ids, &obs, 2, std::slice::from_ref(&r)),
        vec![a.id.clone(), c.id.clone()]
    );
    assert!(select_drop_candidates(&ids, &obs, 0, &[]).is_empty());
}

#[test]
fn extracts_json_from_prose() {
    assert_eq!(
        extract_json_object("Sure:\n{\"ids\": []}\nDone.").unwrap(),
        json!({ "ids": [] })
    );
    assert!(extract_json_object("[1, 2]").is_err());
}
