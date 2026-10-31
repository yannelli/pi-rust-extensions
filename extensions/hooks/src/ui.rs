use std::path::PathBuf;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::config::{ScopeOption, write_option};
use crate::manage::{self, EVENTS};
use crate::model::{Scope, Source};
use crate::panel::{Action, Row, TrustChange, View, run_text, sort_rows};
use crate::runtime::Runtime;

enum Dialog {
    Add(Scope),
    Sources,
    LoadScope,
}

pub async fn hooks_command(rt: Arc<Runtime>, args: String, ctx: CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    rt.reload(host, snap).await;
    if args.trim() == "list" || !host.has_ui() {
        return host.notify_info(listing(&rt, &rows(&rt)).join("\n")).await;
    }
    let trusted = project_trusted(snap);
    let mut view = View::new(trusted);
    let mut select: Option<String> = None;
    loop {
        let mut rows = rows(&rt);
        view.trust_all = rt.trust_all();
        if let Some(hash) = select.take() {
            view.select_hash(&rows, &hash);
        }
        view.clamp(&rows);
        let mut width = snap.terminal_width.map_or(100, |w| w as usize);
        let mut height = panel_height(snap.terminal_height.map(|h| h as usize));
        let mut panel = host.panel(view.render(&rows, width, height)).open().await?;
        width = panel.width().unwrap_or(width);
        let dialog = loop {
            match panel.next_event().await {
                PanelEvent::Closed => return Ok(()),
                PanelEvent::Resize(w) => {
                    width = w;
                    height = panel_height(panel.height()).or(height);
                }
                PanelEvent::Key(key) => match view.key(&key, &rows) {
                    Action::Close => break None,
                    Action::Redraw => {}
                    Action::Add(scope) => break Some(Dialog::Add(scope)),
                    Action::Sources => break Some(Dialog::Sources),
                    Action::LoadScope => break Some(Dialog::LoadScope),
                    action => {
                        view.status = apply(&rt, host, snap, trusted, &rows, action).await;
                        rows = self::rows(&rt);
                        view.trust_all = rt.trust_all();
                        view.clamp(&rows);
                    }
                },
            }
            panel.set_lines(view.render(&rows, width, height))?;
        };
        let _ = panel.close(Value::Null).await;
        let Some(dialog) = dialog else {
            return Ok(());
        };
        view.status = match dialog {
            Dialog::Add(scope) => match add_dialog(&rt, host, snap, trusted, scope).await? {
                Some(Ok(hash)) => {
                    select = Some(hash);
                    Some(format!("Added a {} hook", scope.name()))
                }
                Some(Err(err)) => Some(format!("Cannot add: {err:#}")),
                None => None,
            },
            Dialog::Sources => sources_dialog(&rt, host, snap).await?,
            Dialog::LoadScope => {
                let choice = host
                    .select("Load hooks from", &["global", "project", "both"])
                    .await?;
                match choice {
                    Some(choice) => set_option(&rt, host, snap, "scope", json!(choice)).await,
                    None => None,
                }
            }
        };
    }
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
    let mut rows: Vec<Row> = rt
        .hooks()
        .into_iter()
        .map(|def| {
            let enabled = !rt.is_disabled(&def.hash);
            let (trust, last) = (rt.trust_level(&def.hash), rt.last_run(&def.hash));
            Row::new(def, enabled, trust, last)
        })
        .collect();
    sort_rows(&mut rows);
    rows
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

/// Runs a row action, reloads hooks, and returns the status line.
async fn apply(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    trusted: bool,
    rows: &[Row],
    action: Action,
) -> Option<String> {
    if action == Action::TrustAll {
        let on = !rt
            .lock()
            .loaded
            .as_ref()
            .is_some_and(|l| l.options.trust_all);
        return set_option(rt, host, snap, "trustAll", json!(on)).await;
    }
    let hash = match &action {
        Action::Toggle(h) | Action::Remove(h) | Action::Trust(h, _) => h.clone(),
        _ => return None,
    };
    let row = rows.iter().find(|r| r.def.hash == hash)?;
    let def = &row.def;
    let name = format!("{} {}", def.event, row.matcher());
    let file = scope_file(rt, def.scope)?;
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
            Ok(format!("{name}: {}", rt.trust_level(&hash).name()))
        }
        _ => return None,
    };
    rt.reload(host, snap).await;
    Some(result.unwrap_or_else(|err| format!("Error: {err:#}")))
}

/// Asks for event, matcher, and command; `None` when the user cancels.
async fn add_dialog(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    trusted: bool,
    scope: Scope,
) -> Result<Option<anyhow::Result<String>>> {
    let Some(event) = host.select("Hook event", &EVENTS).await? else {
        return Ok(None);
    };
    let matcher_title = "Matcher: tool name, a|b list, or regex (empty matches all)";
    let Some(matcher) = host.input(matcher_title, Some("Bash")).await? else {
        return Ok(None);
    };
    let Some(command) = host.input("Command to run", Some("./hook.sh")).await? else {
        return Ok(None);
    };
    let Some(file) = scope_file(rt, scope) else {
        return Ok(None);
    };
    let added = manage::add(&file, scope, trusted, &event, Some(&matcher), &command);
    let added = added.map(|def| {
        rt.update_trust(host, |t| t.trust(&def.hash));
        def.hash
    });
    rt.reload(host, snap).await;
    Ok(Some(added))
}

async fn sources_dialog(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot) -> Result<Option<String>> {
    let options = rt
        .lock()
        .loaded
        .as_ref()
        .map(|l| l.options.clone())
        .unwrap_or_default();
    let labels: Vec<String> = Source::ALL
        .iter()
        .map(|s| format!("{}: {}", s.name(), on_off(options.source_on(*s))))
        .collect();
    let Some(choice) = host.select("Toggle which source?", &labels).await? else {
        return Ok(None);
    };
    let Some(source) = Source::ALL
        .into_iter()
        .find(|s| choice.starts_with(s.name()))
    else {
        return Ok(None);
    };
    let key = format!("sources/{}", source.name());
    let on = json!(!options.source_on(source));
    Ok(set_option(rt, host, snap, &key, on).await)
}

/// Writes one key of the global options file and reloads.
async fn set_option(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    key: &str,
    value: Value,
) -> Option<String> {
    let file = scope_file(rt, Scope::Global)?;
    let status = match write_option(&file, key, value.clone()) {
        Ok(()) => format!("Set {key} = {value} in {}", file.display()),
        Err(err) => format!("Error: cannot write {}: {err}", file.display()),
    };
    rt.reload(host, snap).await;
    Some(status)
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
        "Hooks  claude {}  codex {}  pi {}  load {}  trustAll {}{flag}",
        on_off(options.claude),
        on_off(options.codex),
        on_off(options.pi),
        ScopeOption::name(options.scope),
        on_off(options.trust_all),
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
        let state = if row.enabled { "" } else { ", disabled" };
        let last = row.last.as_ref().map_or("never run".into(), run_text);
        let (label, matcher) = (row.def.label(), row.matcher());
        let (summary, trust) = (row.def.summary(), row.trust.name());
        lines.push(format!(
            "   [{label}] {matcher}  {summary}  ({trust}{state})  {last}"
        ));
    }
    lines
}
