use anyhow::Result;
use serde::Serialize;

use crate::env::{Env, Scope};
use crate::market::{self, Located};
use crate::plugin;
use crate::source::PluginSource;
use crate::state::{State, plugin_id};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MarketplaceRow {
    pub name: String,
    pub scope: Scope,
    pub plugin_count: usize,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Installed,
    Available,
    Unavailable,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRow {
    pub marketplace: String,
    pub scope: Scope,
    pub name: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<String>,
    /// `Some` when installed in `scope`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

impl PluginRow {
    pub fn id(&self) -> String {
        plugin_id(&self.name, &self.marketplace)
    }
}

pub struct MarketplacePlugins {
    pub name: String,
    pub scope: Scope,
    pub plugins: Vec<PluginRow>,
}

fn filtered(env: &Env, scope: Option<Scope>) -> Result<Vec<Located>> {
    let all = market::all(env)?;
    Ok(all
        .into_iter()
        .filter(|located| scope.is_none_or(|s| s == located.scope))
        .collect())
}

pub fn marketplaces(env: &Env, scope: Option<Scope>) -> Result<Vec<MarketplaceRow>> {
    Ok(filtered(env, scope)?
        .into_iter()
        .map(|located| MarketplaceRow {
            plugin_count: located.catalog().map(|c| c.plugins.len()).unwrap_or(0),
            name: located.record.name,
            scope: located.scope,
            source: located.record.source,
        })
        .collect())
}

pub fn plugins(
    env: &Env,
    name: Option<&str>,
    scope: Option<Scope>,
) -> Result<Vec<MarketplacePlugins>> {
    let mut states = Vec::new();
    for scope in env.visible_scopes() {
        states.push((scope, State::load(&env.dirs(scope))?));
    }
    let mut out = Vec::new();
    for located in filtered(env, scope)? {
        if name.is_some_and(|n| n != located.record.name) {
            continue;
        }
        let mut rows = Vec::new();
        match located.catalog() {
            Ok(catalog) => {
                for entry in &catalog.plugins {
                    let id = plugin_id(&entry.name, &catalog.name);
                    let installed = states.iter().find_map(|(scope, state)| {
                        let record = state.plugins.get(&id)?;
                        (record.marketplace_scope == located.scope).then_some((*scope, record))
                    });
                    let mut row = PluginRow {
                        marketplace: catalog.name.clone(),
                        scope: located.scope,
                        name: entry.name.clone(),
                        status: Status::Available,
                        version: entry.version.clone(),
                        description: entry.description.clone(),
                        reasons: Vec::new(),
                        enabled: None,
                    };
                    if let Some((scope, record)) = installed {
                        row.scope = scope;
                        row.version = record.version.clone().or(row.version);
                        row.enabled = Some(record.enabled);
                        row.status = if record.enabled {
                            Status::Installed
                        } else {
                            Status::Unavailable
                        };
                        if !record.enabled {
                            row.reasons.push("disabled".into());
                        }
                        row.reasons.extend(
                            record
                                .unsupported
                                .iter()
                                .map(|u| format!("unsupported: {u}")),
                        );
                    } else {
                        match &entry.source {
                            PluginSource::Unsupported(reason) => {
                                row.status = Status::Unavailable;
                                row.reasons.push(reason.clone());
                            }
                            PluginSource::Path(rel) => {
                                match catalog
                                    .local_root(rel)
                                    .and_then(|root| plugin::resolve(entry, &root))
                                {
                                    Ok(resolved) => {
                                        row.version = row.version.or(resolved.version);
                                        row.description = row.description.or(resolved.description);
                                        row.reasons.extend(
                                            resolved
                                                .unsupported
                                                .iter()
                                                .map(|u| format!("unsupported: {u}")),
                                        );
                                    }
                                    Err(reason) => {
                                        row.status = Status::Unavailable;
                                        row.reasons.push(reason);
                                    }
                                }
                            }
                            PluginSource::Git { .. } => row.reasons.push("remote".into()),
                        }
                    }
                    rows.push(row);
                }
            }
            Err(err) => {
                pi_ext_sdk::warn!("cannot read marketplace {}: {err:#}", located.record.name)
            }
        }
        out.push(MarketplacePlugins {
            name: located.record.name,
            scope: located.scope,
            plugins: rows,
        });
    }
    Ok(out)
}

pub fn marketplace_line(row: &MarketplaceRow) -> String {
    format!(
        "[{}] {} -- {} plugin(s) -- {}",
        row.scope, row.name, row.plugin_count, row.source
    )
}

pub fn plugin_line(row: &PluginRow) -> String {
    let tag = match row.status {
        Status::Installed => "[installed]",
        Status::Available => "[available]",
        Status::Unavailable => "[unavailable]",
    };
    let mut parts = vec![format!("  {tag} {}", row.name)];
    parts.extend(row.version.clone());
    if !row.reasons.is_empty() {
        parts.push(format!("({})", row.reasons.join(", ")));
    }
    parts.join("  ")
}

/// Upstream list text and the rows kept by the status filters (no filter keeps all).
pub fn render_plugins(
    all: &[MarketplacePlugins],
    installed: bool,
    available: bool,
    unavailable: bool,
) -> (Vec<String>, Vec<PluginRow>) {
    let any = installed || available || unavailable;
    let keep = |status: Status| {
        !any || match status {
            Status::Installed => installed,
            Status::Available => available,
            Status::Unavailable => unavailable,
        }
    };
    let mut lines = Vec::new();
    let mut rows = Vec::new();
    for mp in all {
        lines.push(format!("Marketplace {} ({})", mp.name, mp.scope));
        if mp.plugins.is_empty() {
            lines.push("  (no plugins)".into());
        }
        for row in mp.plugins.iter().filter(|p| keep(p.status)) {
            lines.push(plugin_line(row));
            rows.push(row.clone());
        }
    }
    (lines, rows)
}
