use std::time::{Duration, Instant};

use pi_ext_sdk::{Error, Result, ToolResult};
use serde::Deserialize;
use serde_json::json;

use crate::act::{Planner, UiAction};
use crate::backend::Kind;
use crate::cond::{Condition, diff};
use crate::live::Live;
use crate::output;
use crate::query::require_state;
use crate::state::Observation;

const POLL: Duration = Duration::from_millis(500);

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ObserveArgs {
    pub root: Option<String>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActArgs {
    #[serde(default)]
    pub state_id: String,
    pub expect: Option<Condition>,
    #[serde(default)]
    pub actions: Vec<UiAction>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WaitArgs {
    #[serde(default)]
    pub state_id: String,
    #[serde(flatten)]
    pub condition: Condition,
}

fn mode(raw: Option<&str>) -> Result<&'static str> {
    match raw.unwrap_or("fused") {
        "fused" => Ok("fused"),
        "semantic" => Ok("semantic"),
        "visual" => Ok("visual"),
        other => Err(Error::msg(format!(
            "mode must be semantic, visual, or fused, not '{other}'."
        ))),
    }
}

/// Rejects mutations from a state that a newer observation of the same root replaced.
pub fn check_current(live: &Live<'_>, base: &Observation) -> Result<()> {
    let store = live.rt.store();
    let latest = store
        .latest_for(&base.target)
        .map(|o| o.capture.state_id.clone());
    match latest {
        Some(latest) if latest != base.capture.state_id => Err(Error::msg(format!(
            "Stale state '{}'. The active operation state is '{latest}'. Observe the root again and retry.",
            base.capture.state_id
        ))),
        _ => Ok(()),
    }
}

impl Live<'_> {
    pub async fn observe(&self, args: ObserveArgs) -> Result<ToolResult> {
        let mode = mode(args.mode.as_deref())?;
        let target = self.resolve(args.root.as_deref()).await?;
        let (observation, image) = self.capture(&target, mode).await?;
        let summary = format!(
            "Observed {mode} {} — {}. Returned the latest outline state.",
            target.app, target.window_title
        );
        Ok(output::observation_result(
            &mut self.rt.store(),
            "observe_ui",
            &summary,
            &observation,
            image,
            None,
        ))
    }

    /// Observes again until `condition` holds or the time limit passes.
    async fn settle(
        &self,
        base: &Observation,
        condition: Option<&Condition>,
        mode: &str,
    ) -> Result<(Observation, Option<crate::backend::Image>, Option<bool>)> {
        let started = Instant::now();
        loop {
            let target = self.refresh(&base.target).await?;
            let target = target.ok_or_else(|| {
                Error::msg("The target window closed. Call find_roots to choose another root.")
            })?;
            let (observation, image) = self.capture(&target, mode).await?;
            let Some(condition) = condition else {
                return Ok((observation, image, None));
            };
            let holds = condition
                .holds(&base.outline.root, &observation.outline.root)
                .map_err(Error::msg)?;
            let elapsed = started.elapsed().as_millis() as u64;
            if holds || elapsed >= condition.timeout_ms() {
                return Ok((observation, image, Some(holds)));
            }
            tokio::select! {
                () = tokio::time::sleep(POLL) => {}
                () = self.cancel.cancelled() => return Err(Error::msg("Operation aborted.")),
            }
        }
    }

    pub async fn act(&self, args: ActArgs) -> Result<ToolResult> {
        let base = require_state(&self.rt.store(), &args.state_id)?;
        if let Some(expect) = &args.expect {
            expect.validate().map_err(Error::msg)?;
        }
        check_current(self, &base)?;
        let config = self.rt.config();
        let outcomes = if base.target.page_id.is_some() {
            crate::page_act::act(&base, &args.actions, self.cancel).await?
        } else {
            let backend = crate::setup::ensure_ready(self.host, self.cancel).await?;
            let mac = matches!(backend.kind, Kind::MacOs { .. });
            let planner = Planner {
                observation: &base,
                semantic_tree: mac,
                headless: config.headless,
                mac,
            };
            let steps = planner.plan(&args.actions).map_err(Error::msg)?;
            backend.act(&base.target, &steps).await?
        };
        let failed = outcomes.iter().find(|o| o["outcome"] != "worked").cloned();
        let image_mode = if base.capture.width > 0 {
            "fused"
        } else {
            "semantic"
        };
        let (next, image, verified) = self.settle(&base, args.expect.as_ref(), image_mode).await?;
        let changes = diff(&base.outline.root, &next.outline.root);
        let outcome = match (&failed, verified) {
            (Some(_), _) | (None, Some(false)) => "didnt",
            (None, Some(true)) => "worked",
            (None, None) => "unknown",
        };
        let mut summary = format!(
            "act_ui outcome: {outcome}. {} step(s) delivered.",
            outcomes.len()
        );
        if let Some(failed) = &failed {
            summary.push_str(&format!(
                " Step {} failed: {}",
                failed["index"],
                failed["error"].as_str().unwrap_or("unknown error")
            ));
        }
        match verified {
            Some(true) => summary.push_str(" expect: verified."),
            Some(false) => summary.push_str(" expect: failed (postcondition_failed)."),
            None => {}
        }
        summary.push_str(&format!("\n{}", changes.summary(20)));
        let mut result = output::observation_result(
            &mut self.rt.store(),
            "act_ui",
            &summary,
            &next,
            image,
            Some(&base.capture.state_id),
        );
        if let Some(details) = result.details.as_mut() {
            details["outcome"] = json!(outcome);
            details["steps"] = json!(outcomes);
            details["changes"] =
                json!({ "added": changes.added.len(), "removed": changes.removed });
        }
        Ok(result)
    }

    pub async fn wait_for(&self, args: WaitArgs) -> Result<ToolResult> {
        let base = require_state(&self.rt.store(), &args.state_id)?;
        args.condition.validate().map_err(Error::msg)?;
        let image_mode = if base.capture.width > 0 {
            "fused"
        } else {
            "semantic"
        };
        let started = Instant::now();
        let (next, image, holds) = self
            .settle(&base, Some(&args.condition), image_mode)
            .await?;
        let elapsed = started.elapsed().as_millis();
        let satisfied = holds == Some(true);
        let summary = if satisfied {
            format!("Condition met after {elapsed}ms.")
        } else {
            format!(
                "Condition was not met within {}ms.",
                args.condition.timeout_ms()
            )
        };
        let mut result = output::observation_result(
            &mut self.rt.store(),
            "wait_for",
            &summary,
            &next,
            image,
            Some(&base.capture.state_id),
        );
        if let Some(details) = result.details.as_mut() {
            details["satisfied"] = json!(satisfied);
        }
        Ok(result)
    }
}
