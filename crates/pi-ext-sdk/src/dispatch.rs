use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::ctx::{Base, CommandCtx, EventCtx, ToolCtx};
use crate::error::{Error, Result};
use crate::handler::BoxFuture;
use crate::json::{merge_patch, merge_patch_diff};
use crate::reply::EventReply;
use crate::server::State;
use crate::snapshot::{InitParams, Snapshot};
use crate::spec::{AutocompleteRequest, RouteRequest};
use crate::warn;

/// How long a cancelled handler may run before the SDK drops it and answers -32800.
const CANCEL_GRACE: Duration = Duration::from_secs(2);

pub(crate) async fn handle(
    state: &Arc<State>,
    method: &str,
    params: Value,
    token: CancellationToken,
) -> Result<Value> {
    let fut: BoxFuture<Value> = match method {
        "initialize" => return initialize(state, params),
        "shutdown" => {
            state.shutdown().await;
            return Ok(Value::Null);
        }
        "event" => {
            let base = base(state, &params, token.clone());
            if params.get("name").and_then(Value::as_str) == Some("session_start") {
                state.start();
            }
            Box::pin(event(base, params))
        }
        "tool.execute" => tool(state, params, token.clone())?,
        "command.run" => command(state, params, token.clone())?,
        "shortcut.run" => shortcut(state, params, token.clone())?,
        "command.complete" => complete(state, params)?,
        "autocomplete.suggest" => autocomplete(state, params)?,
        "virtualModel.route" => route(state, params)?,
        other => return Err(Error::method_not_found(other)),
    };
    guarded(fut, token).await
}

fn initialize(state: &State, params: Value) -> Result<Value> {
    let init: InitParams = serde_json::from_value(params).map_err(Error::invalid_params)?;
    state.host.set_init(init);
    Ok(state
        .host
        .inner
        .registry
        .read()
        .expect("registry lock")
        .initialize_result())
}

fn base(state: &State, params: &Value, cancel: CancellationToken) -> Base {
    let snapshot = match params.get("ctx") {
        Some(ctx) => {
            let snapshot = Arc::new(Snapshot::deserialize_lenient(ctx));
            state.host.set_snapshot(snapshot.clone());
            snapshot
        }
        None => state.host.snapshot().unwrap_or_default(),
    };
    Base {
        host: state.host.clone(),
        snapshot,
        cancel,
    }
}

fn field<T: DeserializeOwned>(params: &Value, key: &str) -> Result<T> {
    let value = params.get(key).cloned().unwrap_or(Value::Null);
    serde_json::from_value(value).map_err(|err| Error::invalid_params(format!("{key}: {err}")))
}

fn to_json<T: Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

async fn event(base: Base, mut params: Value) -> Result<Value> {
    let name: String = field(&params, "name")?;
    let handlers = base
        .host
        .inner
        .registry
        .read()
        .expect("registry lock")
        .event_handlers(&name);
    let original = params
        .get_mut("event")
        .map(Value::take)
        .unwrap_or(Value::Null);
    let mut current = original.clone();
    let (mut result, mut patch, mut first_error, mut succeeded) = (None, None, None, false);
    for handler in &handlers {
        if base.cancel.is_cancelled() {
            return Err(Error::cancelled());
        }
        let ctx = EventCtx {
            base: base.clone(),
            name: name.clone(),
            event: Arc::new(current.clone()),
        };
        match handler(ctx).await {
            Ok(EventReply {
                result: r,
                patch: p,
            }) => {
                succeeded = true;
                if let Some(p) = p {
                    merge_patch(&mut current, &p);
                    patch = Some(p);
                }
                result = r.or(result);
            }
            Err(err) => {
                warn!("handler for {name} failed: {err}");
                first_error.get_or_insert(err);
            }
        }
    }
    if let (false, Some(err)) = (succeeded, first_error) {
        return Err(err);
    }
    if handlers.len() > 1 {
        patch = merge_patch_diff(&original, &current);
    }
    to_json(EventReply { result, patch })
}

fn tool(state: &State, params: Value, token: CancellationToken) -> Result<BoxFuture<Value>> {
    let name: String = field(&params, "name")?;
    let run = state
        .host
        .inner
        .registry
        .read()
        .expect("registry lock")
        .tool(&name);
    let run = run.ok_or_else(|| Error::invalid_params(format!("unknown tool: {name}")))?;
    let ctx = ToolCtx {
        tool_call_id: field::<Option<String>>(&params, "toolCallId")?.unwrap_or_default(),
        base: base(state, &params, token),
    };
    let args = params
        .get("params")
        .cloned()
        .unwrap_or(Value::Object(Default::default()));
    let fut = run(args, ctx);
    Ok(Box::pin(async move { to_json(fut.await?) }))
}

fn command(state: &State, params: Value, token: CancellationToken) -> Result<BoxFuture<Value>> {
    let name: String = field(&params, "name")?;
    let registry = state.host.inner.registry.read().expect("registry lock");
    let run = registry.command(&name).map(|c| c.run.clone());
    drop(registry);
    let run = run.ok_or_else(|| Error::invalid_params(format!("unknown command: {name}")))?;
    let args = field::<Option<String>>(&params, "args")?.unwrap_or_default();
    let fut = run(
        args,
        CommandCtx {
            base: base(state, &params, token),
        },
    );
    Ok(Box::pin(async move { fut.await.map(|()| Value::Null) }))
}

fn shortcut(state: &State, params: Value, token: CancellationToken) -> Result<BoxFuture<Value>> {
    let key: String = field(&params, "shortcut")?;
    let run = state
        .host
        .inner
        .registry
        .read()
        .expect("registry lock")
        .shortcut(&key);
    let run = run.ok_or_else(|| Error::invalid_params(format!("unknown shortcut: {key}")))?;
    let fut = run(CommandCtx {
        base: base(state, &params, token),
    });
    Ok(Box::pin(async move { fut.await.map(|()| Value::Null) }))
}

fn complete(state: &State, params: Value) -> Result<BoxFuture<Value>> {
    let name: String = field(&params, "name")?;
    let prefix = field::<Option<String>>(&params, "prefix")?.unwrap_or_default();
    let registry = state.host.inner.registry.read().expect("registry lock");
    let complete = registry.command(&name).and_then(|c| c.complete.clone());
    drop(registry);
    let Some(complete) = complete else {
        return Ok(Box::pin(std::future::ready(Ok(Value::Null))));
    };
    let fut = complete(prefix, state.host.clone());
    Ok(Box::pin(async move { to_json(fut.await?) }))
}

fn autocomplete(state: &State, params: Value) -> Result<BoxFuture<Value>> {
    let request: AutocompleteRequest =
        serde_json::from_value(params).map_err(Error::invalid_params)?;
    let registry = state.host.inner.registry.read().expect("registry lock");
    let handlers: Vec<_> = registry
        .autocomplete
        .iter()
        .map(|(_, h)| h.clone())
        .collect();
    drop(registry);
    let host = state.host.clone();
    Ok(Box::pin(async move {
        for handler in handlers {
            if let Some(suggestions) = handler(request.clone(), host.clone()).await? {
                return to_json(suggestions);
            }
        }
        Ok(Value::Null)
    }))
}

fn route(state: &State, params: Value) -> Result<BoxFuture<Value>> {
    let request: RouteRequest = serde_json::from_value(params).map_err(Error::invalid_params)?;
    let registry = state.host.inner.registry.read().expect("registry lock");
    let route = registry.route(&request.provider, &request.id);
    drop(registry);
    let missing = || {
        Error::invalid_params(format!(
            "unknown virtual model: {}/{}",
            request.provider, request.id
        ))
    };
    let route = route.ok_or_else(missing)?;
    Ok(route(request, state.host.clone()))
}

/// Runs a handler in its own task: panics become -32603, and a cancelled handler that
/// fails or outlives [`CANCEL_GRACE`] answers -32800.
async fn guarded(fut: BoxFuture<Value>, token: CancellationToken) -> Result<Value> {
    let mut task = tokio::spawn(fut);
    let joined = tokio::select! {
        joined = &mut task => joined,
        () = token.cancelled() => match tokio::time::timeout(CANCEL_GRACE, &mut task).await {
            Ok(joined) => joined,
            Err(_) => {
                task.abort();
                return Err(Error::cancelled());
            }
        },
    };
    match joined {
        Ok(Err(_)) if token.is_cancelled() => Err(Error::cancelled()),
        Ok(result) => result,
        Err(join) if join.is_panic() => {
            let panic = join.into_panic();
            let text = panic
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_owned());
            Err(Error::msg(format!("handler panicked: {text}")))
        }
        Err(_) => Err(Error::cancelled()),
    }
}
