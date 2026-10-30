//! Starts child runs: run registry entries, the concurrency limit, and `todo:scope` events.

use std::path::PathBuf;
use std::sync::Arc;

use pi_ext_sdk::CancellationToken;
use pi_ext_sdk::prelude::*;
use tokio::sync::{Notify, Semaphore, mpsc};
use tokio::task::JoinHandle;

use crate::agent::AgentDef;
use crate::app::App;
use crate::command::{self, ChildInput, Inherit, Overrides};
use crate::exec;
use crate::runs::Status;
use crate::stream::Effect;

pub const TODO_SCOPE_CHANNEL: &str = "todo:scope";

#[derive(Debug, Clone)]
pub struct Job {
    pub agent: AgentDef,
    pub task: String,
    pub overrides: Overrides,
    pub cwd: PathBuf,
}

pub struct Launch {
    pub program: PathBuf,
    pub parent: Inherit,
    pub extra_args: Vec<String>,
    pub keep: usize,
    pub limit: Arc<Semaphore>,
    /// Woken on each progress change of any run of this launch.
    pub changed: Arc<Notify>,
}

pub struct Started {
    pub id: String,
    pub handle: JoinHandle<Status>,
}

pub fn start(
    app: &App,
    host: &Host,
    launch: &Launch,
    job: Job,
    background: bool,
    cancel: CancellationToken,
) -> Started {
    let id = app
        .runs
        .start(&job.agent.name, &job.task, background, cancel.clone());
    let spec = command::build(ChildInput {
        agent: &job.agent,
        task: &job.task,
        run_id: &id,
        parent: &launch.parent,
        overrides: &job.overrides,
        program: &launch.program,
        cwd: &job.cwd,
        extra_args: &launch.extra_args,
    });
    let label = format!(
        "{}: {}",
        job.agent.name,
        crate::runs::first_line(&job.task, 60)
    );
    let (scope_tx, scope_rx) = mpsc::unbounded_channel::<Value>();
    tokio::spawn(forward_scope(host.clone(), scope_rx));
    let scope_id = id.clone();
    let scope = move |status: &str, state: Value| json!({ "scope": scope_id, "label": label, "status": status, "state": state });
    let _ = scope_tx.send(scope("running", Value::Null));
    let runs = app.runs.clone();
    let limit = launch.limit.clone();
    let changed = launch.changed.clone();
    let keep = launch.keep;
    let run_id = id.clone();
    let handle = tokio::spawn(async move {
        let permit = tokio::select! {
            permit = limit.acquire_owned() => permit.ok(),
            _ = cancel.cancelled() => None,
        };
        let status = match permit {
            Some(_permit) => {
                let mut on_effect = |effect: Effect| {
                    if let Effect::Todo(details) = effect {
                        let _ = scope_tx.send(scope("running", details));
                    }
                    changed.notify_one();
                };
                exec::execute(&spec, &runs, &run_id, &cancel, &mut on_effect).await
            }
            None => Status::Cancelled,
        };
        runs.finish(&run_id, status, keep);
        let _ = scope_tx.send(scope(status.as_str(), Value::Null));
        changed.notify_one();
        status
    });
    Started { id, handle }
}

async fn forward_scope(host: Host, mut rx: mpsc::UnboundedReceiver<Value>) {
    while let Some(payload) = rx.recv().await {
        if let Err(err) = host.emit(TODO_SCOPE_CHANNEL, payload).await {
            pi_ext_sdk::debug!("todo:scope emit failed: {}", err.message());
        }
    }
}
