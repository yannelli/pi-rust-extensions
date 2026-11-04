//! Judgments for other extensions over `pi.events`, under the same gate and limits as the tool.

use std::sync::Arc;

use pi_ext_sdk::CancellationToken;
use pi_ext_sdk::prelude::*;

use crate::errors::TsError;
use crate::state::State;
use crate::{call_out_auth, validate};

pub const REQUEST_CHANNEL: &str = "typesafe:decision:request";
pub const REPLY_CHANNEL: &str = "typesafe:decision:reply";

/// Answers each `{ id, request }` on the request channel with one reply carrying the same `id`.
pub async fn serve(state: Arc<State>, host: Host) -> Result<()> {
    let mut requests = host.subscribe(REQUEST_CHANNEL).await?;
    while let Some(message) = requests.recv().await {
        let (state, host) = (state.clone(), host.clone());
        tokio::spawn(async move { answer(&state, &host, message).await });
    }
    Ok(())
}

async fn answer(state: &State, host: &Host, message: Value) {
    let Some(id) = message.get("id").and_then(Value::as_str) else {
        return;
    };
    let request = message.get("request").cloned().unwrap_or(Value::Null);
    let reply = match decide(state, host, request).await {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err((code, error)) => json!({ "id": id, "ok": false, "code": code, "error": error }),
    };
    if let Err(error) = host.emit(REPLY_CHANNEL, reply).await {
        pi_ext_sdk::warn!(
            "typesafe: could not reply on {REPLY_CHANNEL}: {}",
            error.message()
        );
    }
}

async fn decide(
    state: &State,
    host: &Host,
    request: Value,
) -> std::result::Result<Value, (&'static str, String)> {
    if !state.enabled() {
        let text = "TypeSafe is disabled. Ask the operator to run /typesafe enable.";
        return Err(("disabled", text.to_owned()));
    }
    if let Some(error) = &state.backend_error {
        return Err(("configuration", error.clone()));
    }
    let fail = |error: TsError| (error.code.as_str(), error.message);
    let request = validate::prepare(request, None).map_err(fail)?;
    let outcome = match state.get_client() {
        Ok(client) => client.evaluate(request, &CancellationToken::new()).await,
        Err(error) => Err(error),
    };
    match outcome {
        Ok(result) => Ok(result),
        Err(error) => {
            call_out_auth(state, host, &error).await;
            Err(fail(error))
        }
    }
}
