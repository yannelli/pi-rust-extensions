//! The `conflicts` option: what runs when hooks from several layers match one event and matcher.

use std::cmp::Reverse;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use pi_ext_sdk::Host;

use crate::config::codex_plugin_bundle::bundle_unchanged;
use crate::dispatch::NO_MATCHER;
use crate::model::{HookDef, Layer, Source, canonical};
use crate::output::{Outcome, Permission};
use crate::runtime::Runtime;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConflictMode {
    /// Every layer runs, as in Claude Code and Codex.
    #[default]
    Merge,
    /// Every layer runs, and each conflict shows a warning once per session.
    Warn,
    /// The most specific layer runs; the next layer runs when none of its hooks can.
    Override,
}

impl ConflictMode {
    pub const ALL: [ConflictMode; 3] = [Self::Merge, Self::Warn, Self::Override];

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.name() == text)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Warn => "warn",
            Self::Override => "override",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Merge => "merge: run the hooks of all layers (the Claude Code and Codex default)",
            Self::Warn => "warn: run the hooks of all layers and show a warning for each conflict",
            Self::Override => {
                "override: run only the most specific layer (local, then project, then global)"
            }
        }
    }
}

/// Hooks of one family (Codex, or Claude and pi) for one event and exact matcher.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConflictKey {
    pub codex: bool,
    pub event: String,
    pub matcher: String,
}

/// Events that ignore the matcher key as `*`.
pub fn conflict_key(def: &HookDef) -> ConflictKey {
    let matcher = def
        .matcher
        .as_deref()
        .filter(|m| !m.is_empty() && !NO_MATCHER.contains(&def.event.as_str()));
    ConflictKey {
        codex: def.source.is_codex(),
        event: def.event.clone(),
        matcher: matcher.unwrap_or("*").to_owned(),
    }
}

/// The same handler runs once per event; a plugin's copy stays separate.
fn dedupe_key(def: &HookDef) -> (bool, Option<String>, String) {
    let plugin = def.plugin.as_ref().and_then(|p| p.id.clone());
    (
        def.source.is_codex(),
        plugin,
        canonical(&def.handler).to_string(),
    )
}

/// Different handlers for one key from more than one layer.
#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub key: ConflictKey,
    pub layers: Vec<Layer>,
    pub hashes: Vec<String>,
}

impl Conflict {
    pub fn text(&self) -> String {
        let family = if self.key.codex { "Codex" } else { "Claude/pi" };
        let mut names: Vec<&str> = self.layers.iter().map(|l| l.name()).collect();
        names.dedup();
        format!(
            "{} {} ({family}): {}",
            self.key.event,
            self.key.matcher,
            names.join(", ")
        )
    }
}

pub fn find_conflicts(hooks: &[HookDef]) -> Vec<Conflict> {
    let mut groups: BTreeMap<ConflictKey, Vec<&HookDef>> = BTreeMap::new();
    for def in hooks {
        groups.entry(conflict_key(def)).or_default().push(def);
    }
    groups
        .into_iter()
        .filter_map(|(key, defs)| {
            let handlers: HashSet<_> = defs.iter().map(|d| dedupe_key(d)).collect();
            let mut layers: Vec<Layer> = defs.iter().map(|d| d.layer).collect();
            layers.sort();
            layers.dedup();
            (layers.len() > 1 && handlers.len() > 1).then(|| Conflict {
                key,
                layers,
                hashes: defs.iter().map(|d| d.hash.clone()).collect(),
            })
        })
        .collect()
}

/// Command hooks that Claude or pi config and Codex config both register for one event; both run.
pub fn cross_source_duplicates(hooks: &[HookDef]) -> Vec<String> {
    let mut seen: BTreeMap<(String, String), [bool; 2]> = BTreeMap::new();
    for def in hooks.iter().filter(|d| d.kind() == "command") {
        let entry = seen.entry((def.event.clone(), def.summary())).or_default();
        entry[usize::from(def.source.is_codex())] = true;
    }
    seen.into_iter()
        .filter(|(_, [claude, codex])| *claude && *codex)
        .map(|((event, command), _)| format!("{event}: {command} (Claude/pi and Codex)"))
        .collect()
}

/// Under `override`, the hooks that an enabled hook of a more specific layer replaces.
pub fn shadowed(hooks: &[HookDef], active: impl Fn(&HookDef) -> bool) -> HashSet<String> {
    let ordered = |def: &&HookDef| def.layer != Layer::Managed;
    let mut top: HashMap<ConflictKey, Layer> = HashMap::new();
    for def in hooks.iter().filter(ordered).filter(|d| active(d)) {
        let layer = top.entry(conflict_key(def)).or_insert(def.layer);
        *layer = (*layer).max(def.layer);
    }
    hooks
        .iter()
        .filter(ordered)
        .filter(|def| top.get(&conflict_key(def)).is_some_and(|l| *l > def.layer))
        .map(|def| def.hash.clone())
        .collect()
}

/// The matched hooks that run, least specific layer first, so last-wins outputs take the most
/// specific hook. Trust resolves from the most specific layer down, so a skipped copy has a stand-in.
pub async fn runnable(
    rt: &Runtime,
    host: &Host,
    selected: Vec<HookDef>,
    mode: ConflictMode,
) -> Vec<HookDef> {
    let mut order: Vec<usize> = (0..selected.len()).collect();
    order.sort_by_key(|&i| Reverse(selected[i].layer));
    let mut handlers = HashSet::new();
    let mut winners: HashMap<ConflictKey, Layer> = HashMap::new();
    let mut keep = vec![false; selected.len()];
    let mut fingerprints: HashMap<PathBuf, Option<String>> = HashMap::new();
    for i in order {
        let def = &selected[i];
        let handler = dedupe_key(def);
        if handlers.contains(&handler) {
            continue;
        }
        let ordered = mode == ConflictMode::Override && def.layer != Layer::Managed;
        let key = conflict_key(def);
        if ordered && winners.get(&key).is_some_and(|layer| *layer != def.layer) {
            continue;
        }
        if !may_run(rt, host, def, &mut fingerprints).await {
            continue;
        }
        handlers.insert(handler);
        if ordered {
            winners.entry(key).or_insert(def.layer);
        }
        keep[i] = true;
    }
    let mut kept: Vec<HookDef> = selected
        .into_iter()
        .zip(keep)
        .filter_map(|(def, keep)| keep.then_some(def))
        .collect();
    kept.sort_by_key(|def| def.layer);
    kept
}

async fn may_run(
    rt: &Runtime,
    host: &Host,
    def: &HookDef,
    fingerprints: &mut HashMap<PathBuf, Option<String>>,
) -> bool {
    if !bundle_unchanged(def, fingerprints) {
        return false;
    }
    if def.layer != Layer::Managed && !rt.resolve_trust(host, def).await {
        return false;
    }
    let once = def.source == Source::Pi && def.bool_field("once");
    !(once && rt.lock().ran_once.contains(&def.hash))
}

/// Under `warn`: a notice when tool hooks return different decisions or several rewrite the input.
pub fn disagreement(event: &str, tool: &str, outcomes: &[Outcome]) -> Option<String> {
    let mut decisions: Vec<Permission> = outcomes
        .iter()
        .filter_map(|o| o.decision().map(|(permission, _)| permission))
        .filter(|p| *p != Permission::Defer)
        .collect();
    decisions.sort();
    decisions.dedup();
    let rewrites = outcomes
        .iter()
        .filter(|o| o.updated_input.is_some())
        .count();
    let mut parts = Vec::new();
    if let Some(winner) = decisions.last().filter(|_| decisions.len() > 1) {
        let names: Vec<String> = decisions.iter().map(|p| permission_name(*p)).collect();
        let winner = permission_name(*winner);
        parts.push(format!(
            "the decisions are different ({}), and {winner} applies",
            names.join(", ")
        ));
    }
    if rewrites > 1 {
        parts.push(format!(
            "{rewrites} hooks changed the input, and the most specific change applies"
        ));
    }
    (!parts.is_empty()).then(|| format!("hooks: {event} hooks for {tool}: {}.", parts.join("; ")))
}

fn permission_name(permission: Permission) -> String {
    format!("{permission:?}").to_lowercase()
}

#[cfg(test)]
mod tests;
