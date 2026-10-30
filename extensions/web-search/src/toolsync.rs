//! Removes `url_context` from the active tools while no provider can serve it and restores
//! it afterwards, keeping tools the user turned on or off in between.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use pi_ext_sdk::{Host, Snapshot};

use crate::context::{current_model, load_config};
use crate::model::SearchModel;
use crate::plan::url_context_available;
use crate::tools::URL_CONTEXT;

#[derive(Debug, Default)]
pub struct ToolSync {
    preferred: Option<BTreeSet<String>>,
    last_applied: Option<BTreeSet<String>>,
    suppressed: BTreeSet<String>,
}

pub type Shared = Arc<Mutex<ToolSync>>;

impl ToolSync {
    /// The new active set when it differs from `current`.
    pub fn step(
        &mut self,
        current: &BTreeSet<String>,
        supported: bool,
    ) -> Option<BTreeSet<String>> {
        let preferred = self.preferred.get_or_insert_with(|| current.clone());
        if let Some(last) = &self.last_applied {
            preferred.extend(current.difference(last).cloned());
            for tool in last.difference(current) {
                if !self.suppressed.contains(tool) {
                    preferred.remove(tool);
                }
            }
        }
        let mut desired = preferred.clone();
        self.suppressed.clear();
        if !supported {
            desired.remove(URL_CONTEXT);
            if preferred.contains(URL_CONTEXT) {
                self.suppressed.insert(URL_CONTEXT.to_owned());
            }
        }
        self.last_applied = Some(desired.clone());
        (desired != *current).then_some(desired)
    }
}

/// `model` comes from a `model_select` event; otherwise pi is asked for the current model.
pub async fn sync(
    state: &Shared,
    host: &Host,
    snapshot: &Snapshot,
    model: Option<SearchModel>,
) -> pi_ext_sdk::Result<()> {
    let model = match model {
        Some(model) => Some(model),
        None => current_model(host).await,
    };
    let loaded = load_config(host, Some(snapshot)).await;
    let supported = url_context_available(&host.agent_dir(), &loaded, model.as_ref());
    let current: BTreeSet<String> = host.active_tools().await?.into_iter().collect();
    let desired = state
        .lock()
        .expect("tool sync lock")
        .step(&current, supported);
    if let Some(desired) = desired {
        let names: Vec<String> = desired.into_iter().collect();
        host.set_active_tools(&names).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn suppresses_and_restores_url_context() {
        let mut sync = ToolSync::default();
        let all = set(&["read", "url_context", "web_search"]);
        let without = set(&["read", "web_search"]);
        assert_eq!(sync.step(&all, false), Some(without.clone()));
        assert_eq!(sync.step(&without, false), None);
        assert_eq!(sync.step(&without, true), Some(all.clone()));
        assert_eq!(sync.step(&all, true), None);
    }

    #[test]
    fn keeps_user_toggles() {
        let mut sync = ToolSync::default();
        let all = set(&["read", "url_context", "web_search"]);
        sync.step(&all, true);
        let user = set(&["read", "web_search", "bash"]);
        assert_eq!(sync.step(&user, true), None);
        assert_eq!(sync.step(&user, false), None);
    }
}
