use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::config::{ScopeOption, write_option};
use crate::manage;
use crate::model::{HookDef, Layer, Scope};
use crate::panel::{Action, Row, Tone, TrustChange, View, run_text, sort_rows};
use crate::policy::{self, ConflictMode};
use crate::runtime::Runtime;
use crate::trust::Trust;

pub async fn hooks_command(rt: Arc<Runtime>, args: String, ctx: CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    rt.reload(host, snap).await;
    if args.trim() == "list" || !host.has_ui() {
        return host.notify_info(listing(&rt, &rows(&rt)).join("\n")).await;
    }
    let trusted = project_trusted(snap);
    let mut view = View::new(trusted);
    let mut rows = rows(&rt);
    refresh(&rt, &mut view, &rows);
    let mut width = snap.terminal_width.map_or(100, |w| w as usize);
    let mut height = panel_height(snap.terminal_height.map(|h| h as usize));
    let mut panel = host.panel(view.render(&rows, width, height)).open().await?;
    width = panel.width().unwrap_or(width);
    loop {
        match panel.next_event().await {
            PanelEvent::Closed => return Ok(()),
            PanelEvent::Resize(w) => {
                width = w;
                height = panel_height(panel.height()).or(height);
            }
            PanelEvent::Key(key) => match view.key(&key, &rows) {
                Action::Close => {
                    let _ = panel.close(Value::Null).await;
                    return Ok(());
                }
                Action::Redraw => {}
                action => {
                    let (status, select) = apply(&rt, host, snap, trusted, &rows, action).await;
                    view.status = status;
                    rows = self::rows(&rt);
                    refresh(&rt, &mut view, &rows);
                    if let Some(hash) = select {
                        view.select_hash(&rows, &hash);
                    }
                }
            },
        }
        view.clamp(&rows);
        panel.set_lines(view.render(&rows, width, height))?;
    }
}

/// Copies the options and the trust flag the panel shows.
fn refresh(rt: &Runtime, view: &mut View, rows: &[Row]) {
    let state = rt.lock();
    view.options = state
        .loaded
        .as_ref()
        .map(|l| l.options.clone())
        .unwrap_or_default();
    view.trust_flag = state.flag_trust_all;
    drop(state);
    view.clamp(rows);
}

/// Rows pi keeps outside a panel: the cwd and usage footer lines, and the top row of
/// fullscreen mode. The ctx does not say which TUI mode runs, so both modes reserve it.
const HOST_ROWS: usize = 3;

/// The lines the panel may use when pi reports `terminal_rows`.
fn panel_height(terminal_rows: Option<usize>) -> Option<usize> {
    terminal_rows.map(|rows| rows.saturating_sub(HOST_ROWS).max(1))
}

fn project_trusted(snap: &Snapshot) -> bool {
    snap.extra
        .get("isProjectTrusted")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn rows(rt: &Runtime) -> Vec<Row> {
    let hooks = rt.hooks();
    let enabled = |def: &HookDef| def.layer == Layer::Managed || !rt.is_disabled(&def.hash);
    let active = |def: &HookDef| enabled(def) && rt.trust_of(def) != Trust::Denied;
    let shadowed = match rt.conflict_mode() {
        ConflictMode::Override => policy::shadowed(&hooks, active),
        _ => HashSet::new(),
    };
    let conflicts = rt
        .lock()
        .loaded
        .as_ref()
        .map(|l| l.conflicts.clone())
        .unwrap_or_default();
    let mut rows: Vec<Row> = hooks
        .iter()
        .map(|def| {
            let (trust, last) = (rt.trust_of(def), rt.last_run(&def.hash));
            let mut row = Row::new(def.clone(), enabled(def), trust, last);
            row.shadowed = shadowed.contains(&def.hash);
            row.conflict = conflicts
                .iter()
                .find(|c| c.hashes.contains(&def.hash))
                .map(|c| conflict_text(&hooks, def, &c.hashes));
            row
        })
        .collect();
    sort_rows(&mut rows);
    rows
}

/// The other layers of a conflict: `project .claude/settings.json, global ~/.codex/hooks.json`.
fn conflict_text(hooks: &[HookDef], def: &HookDef, hashes: &[String]) -> String {
    let mut others: Vec<String> = hooks
        .iter()
        .filter(|other| other.layer != def.layer && hashes.contains(&other.hash))
        .map(|other| format!("{} {}", other.layer.name(), other.file.display()))
        .collect();
    others.sort();
    others.dedup();
    format!("also defined in {}", others.join(", "))
}

/// The pi options file that holds the `disabled` list of `scope`.
fn scope_file(rt: &Runtime, scope: Scope) -> Option<PathBuf> {
    let state = rt.lock();
    let loaded = state.loaded.as_ref()?;
    Some(match scope {
        Scope::Global => loaded.global_file.clone(),
        Scope::Project => loaded.project_file.clone(),
    })
}

/// Runs a panel action and reloads hooks. Returns the status line and the hook to select.
async fn apply(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    trusted: bool,
    rows: &[Row],
    action: Action,
) -> (Option<(Tone, String)>, Option<String>) {
    let (result, select) = match action {
        Action::Set(setting, index) => {
            let value = setting.json(index);
            (set_option(rt, &setting.key(), value), None)
        }
        Action::Add {
            scope,
            event,
            matcher,
            command,
        } => {
            let Some(file) = scope_file(rt, scope) else {
                return (None, None);
            };
            match manage::add(&file, scope, trusted, &event, Some(&matcher), &command) {
                Ok(def) => {
                    rt.update_trust(host, |t| t.trust(&def.hash));
                    let text = format!(
                        "Added a {} {event} hook to {}",
                        scope.name(),
                        file.display()
                    );
                    (Ok(text), Some(def.hash))
                }
                Err(err) => (Err(format!("Cannot add: {err:#}")), None),
            }
        }
        action => (row_action(rt, host, trusted, rows, action), None),
    };
    rt.reload(host, snap).await;
    let status = match result {
        Ok(text) => (Tone::Info, text),
        Err(text) => (Tone::Warn, text),
    };
    (Some(status), select)
}

/// Toggle, trust changes, and remove on one hook.
fn row_action(
    rt: &Runtime,
    host: &Host,
    trusted: bool,
    rows: &[Row],
    action: Action,
) -> std::result::Result<String, String> {
    let hash = match &action {
        Action::Toggle(h) | Action::Remove(h) | Action::Trust(h, _) => h.clone(),
        _ => return Err(String::new()),
    };
    let row = rows
        .iter()
        .find(|r| r.def.hash == hash)
        .ok_or_else(|| "The hook is gone; the list was reloaded.".to_owned())?;
    let def = &row.def;
    if def.layer == Layer::Managed {
        return Err(format!(
            "{} is a managed hook from {}. Change it in that policy file.",
            def.event,
            def.file.display()
        ));
    }
    let name = format!("{} {}", def.event, row.matcher());
    let file = scope_file(rt, def.scope).ok_or_else(String::new)?;
    let result = match action {
        Action::Toggle(_) if row.enabled => manage::set_disabled(&file, def, true, trusted)
            .map(|()| format!("Disabled {name}; saved in {}", file.display())),
        Action::Toggle(_) => manage::set_disabled(&file, def, false, trusted).map(|()| {
            let legacy = rt
                .lock()
                .trust
                .as_ref()
                .is_some_and(|t| t.disabled.contains(&hash));
            if legacy {
                rt.update_trust(host, |t| {
                    t.disabled.remove(&hash);
                });
            }
            format!("Enabled {name}")
        }),
        Action::Remove(_) => manage::remove(def, trusted).and_then(|()| {
            manage::set_disabled(&file, def, false, trusted)?;
            rt.update_trust(host, |t| {
                t.untrust(&hash);
                t.disabled.remove(&hash);
            });
            Ok(format!("Removed {name} from {}", def.file.display()))
        }),
        Action::Trust(_, change) => {
            match change {
                TrustChange::Always => rt.update_trust(host, |t| t.trust(&hash)),
                TrustChange::Once => {
                    rt.lock().once_allowed.insert(hash.clone());
                }
                TrustChange::Untrust => {
                    rt.update_trust(host, |t| t.untrust(&hash));
                    rt.lock().once_allowed.remove(&hash);
                }
                TrustChange::Deny => rt.update_trust(host, |t| t.deny(&hash)),
            }
            Ok(format!("{name}: {}", rt.trust_of(def).name()))
        }
        _ => return Err(String::new()),
    };
    result.map_err(|err| format!("Error: {err:#}"))
}

/// Writes one key of the global options file; the caller reloads.
fn set_option(rt: &Runtime, key: &str, value: Value) -> std::result::Result<String, String> {
    let file = scope_file(rt, Scope::Global).ok_or_else(String::new)?;
    match write_option(&file, key, value.clone()) {
        Ok(()) => Ok(format!("Set {key} = {value} in {}", file.display())),
        Err(err) => Err(format!("Error: cannot write {}: {err}", file.display())),
    }
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

/// The `/hooks list` text.
fn listing(rt: &Runtime, rows: &[Row]) -> Vec<String> {
    let options = rt
        .lock()
        .loaded
        .as_ref()
        .map(|l| l.options.clone())
        .unwrap_or_default();
    let flag = if rt.lock().flag_trust_all {
        " (flag on)"
    } else {
        ""
    };
    let mut lines = vec![format!(
        "Hooks  claude {}  codex {}  pi {}  load {}  trustAll {}{flag}  conflicts {}",
        on_off(options.claude),
        on_off(options.codex),
        on_off(options.pi),
        ScopeOption::name(options.scope),
        on_off(options.trust_all),
        options.conflicts.name(),
    )];
    if rows.is_empty() {
        lines.push("No hooks found.".to_owned());
    }
    let mut sorted: Vec<&Row> = rows.iter().collect();
    sorted.sort_by_key(|r| (&r.def.event, r.def.source, r.def.scope));
    let mut event = "";
    for row in sorted {
        if row.def.event != event {
            event = &row.def.event;
            lines.push(event.to_owned());
        }
        let mut state = if row.enabled { "" } else { ", disabled" }.to_owned();
        if let Some(tag) = row.tag() {
            state.push_str(&format!(", {tag}"));
        }
        let last = row.last.as_ref().map_or("never run".into(), run_text);
        let (label, matcher) = (row.def.label(), row.matcher());
        let (summary, trust) = (row.def.summary(), row.trust.name());
        lines.push(format!(
            "   [{label}] {matcher}  {summary}  ({trust}{state})  {last}"
        ));
    }
    lines
}
