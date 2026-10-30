use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::cdp::Cdp;
use crate::state::DEFAULT_TIMEOUT;

pub struct Page {
    cdp: Arc<Cdp>,
    session: String,
    pub target_id: String,
}

impl Page {
    pub fn new(cdp: Arc<Cdp>, session: String, target_id: String) -> Self {
        Self {
            cdp,
            session,
            target_id,
        }
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.cdp
            .call(method, params, Some(&self.session), DEFAULT_TIMEOUT)
            .await
    }

    pub async fn call_with(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.cdp
            .call(method, params, Some(&self.session), timeout)
            .await
    }

    /// `Runtime.evaluate` by value; a thrown exception is an error.
    pub async fn evaluate(&self, expression: &str, await_promise: bool) -> Result<Value> {
        let params = json!({
            "expression": expression, "returnByValue": true,
            "awaitPromise": await_promise, "userGesture": true
        });
        let reply = self.call("Runtime.evaluate", params).await?;
        if let Some(details) = reply.get("exceptionDetails") {
            let text = details
                .pointer("/exception/description")
                .or_else(|| details.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("exception");
            bail!("JavaScript error: {text}");
        }
        Ok(reply
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    pub async fn detach(self) {
        let params = json!({ "sessionId": self.session });
        let _ = (self.cdp)
            .call(
                "Target.detachFromTarget",
                params,
                None,
                Duration::from_secs(2),
            )
            .await;
    }
}
