use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use pi_ext_sdk::prelude::*;

use crate::config::{ScopeOption, write_option};
use crate::model::{HookDef, Scope, Source};
use crate::runtime::Runtime;
use crate::trust::LastRun;

const HELP: &str =
    "up/down move  enter actions  s sources  c scope  t trustAll  g/p config file  q close";

enum Action {
    Hook(Box<HookDef>),
    Sources,
    Scope,
}

pub async fn hooks_command(rt: Arc<Runtime>, args: String, ctx: CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    rt.reload(host, snap).await;
    if args.trim() == "list" || !host.has_ui() {
        return host
            .notify_info(listing(&rt, &sorted(&rt), None, Scope::Global).join("\n"))
            .await;
    }
    let mut selected = 0usize;
    let mut target = Scope::Global;
    loop {
        let hooks = sorted(&rt);
        selected = selected.min(hooks.len().saturating_sub(1));
        let render = |selected: usize, target: Scope| listing(&rt, &hooks, Some(selected), target);
        let mut panel = host.panel(render(selected, target)).open().await?;
        let action = loop {
            let key = match panel.next_event().await {
                PanelEvent::Key(key) => key,
                PanelEvent::Closed => return Ok(()),
                PanelEvent::Resize(_) => {
                    panel.set_lines(render(selected, target))?;
                    continue;
                }
            };
            match key.char() {
                _ if key.is_cancel() => break None,
                _ if key.is("up") => selected = selected.saturating_sub(1),
                _ if key.is("down") => selected = (selected + 1).min(hooks.len().saturating_sub(1)),
                _ if key.is("enter") && !hooks.is_empty() => {
                    break Some(Action::Hook(Box::new(hooks[selected].clone())));
                }
                Some('s') => break Some(Action::Sources),
                Some('c') => break Some(Action::Scope),
                Some('t') => {
                    let on = !rt
                        .lock()
                        .loaded
                        .as_ref()
                        .is_some_and(|l| l.options.trust_all);
                    set_option(&rt, host, snap, Scope::Global, "trustAll", json!(on)).await;
                }
                Some('g') => target = Scope::Global,
                Some('p') => target = Scope::Project,
                _ => {}
            }
            panel.set_lines(render(selected, target))?;
        };
        let _ = panel.close(Value::Null).await;
        match action {
            None => return Ok(()),
            Some(Action::Hook(def)) => hook_menu(&rt, host, snap, &def).await?,
            Some(Action::Sources) => {
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
                if let Some(choice) = host.select("Toggle which source?", &labels).await? {
                    let source = Source::ALL
                        .iter()
                        .find(|s| choice.starts_with(s.name()))
                        .copied();
                    if let Some(source) = source {
                        let key = format!("sources/{}", source.name());
                        set_option(
                            &rt,
                            host,
                            snap,
                            target,
                            &key,
                            json!(!options.source_on(source)),
                        )
                        .await;
                    }
                }
            }
            Some(Action::Scope) => {
                if let Some(choice) = host
                    .select("Load hooks from", &["global", "project", "both"])
                    .await?
                {
                    set_option(&rt, host, snap, target, "scope", json!(choice)).await;
                }
            }
        }
    }
}

async fn hook_menu(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, def: &HookDef) -> Result<()> {
    let disabled = rt.is_disabled(&def.hash);
    let toggle = if disabled { "Enable" } else { "Disable" };
    let options = [
        "Trust always",
        "Allow once",
        "Untrust",
        "Deny",
        toggle,
        "Show last output",
    ];
    let Some(choice) = host.select(&def.trust_text(), &options).await? else {
        return Ok(());
    };
    match choice.as_str() {
        "Trust always" => rt.update_trust(host, |t| t.trust(&def.hash)),
        "Allow once" => {
            rt.lock().once_allowed.insert(def.hash.clone());
        }
        "Untrust" => {
            rt.update_trust(host, |t| t.untrust(&def.hash));
            rt.lock().once_allowed.remove(&def.hash);
        }
        "Deny" => rt.update_trust(host, |t| t.deny(&def.hash)),
        "Show last output" => {
            let text = match rt.last_run(&def.hash) {
                Some(run) => format!(
                    "{}\n\nstdout:\n{}\n\nstderr:\n{}",
                    run_text(&run),
                    run.stdout,
                    run.stderr
                ),
                None => "This hook has not run yet.".to_owned(),
            };
            host.editor(&format!("Last output: {def}"), Some(&text))
                .await?;
        }
        _ => {
            rt.update_trust(host, |t| {
                t.toggle_disabled(&def.hash);
            });
        }
    }
    rt.reload(host, snap).await;
    Ok(())
}

async fn set_option(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    target: Scope,
    key: &str,
    value: Value,
) {
    let file = {
        let state = rt.lock();
        let loaded = state.loaded.as_ref();
        let pick = |l: &crate::config::Loaded| {
            if target == Scope::Global {
                l.global_file.clone()
            } else {
                l.project_file.clone()
            }
        };
        loaded.map(pick)
    };
    let Some(file) = file else {
        return;
    };
    if let Err(err) = write_option(&file, key, value) {
        let _ = host
            .notify_error(format!("hooks: cannot write {}: {err}", file.display()))
            .await;
    }
    rt.reload(host, snap).await;
}

fn sorted(rt: &Runtime) -> Vec<HookDef> {
    let mut hooks = rt.hooks();
    hooks.sort_by(|a, b| (&a.event, a.source, a.scope).cmp(&(&b.event, b.source, b.scope)));
    hooks
}

fn on_off(on: bool) -> &'static str {
    if on { "on" } else { "off" }
}

fn run_text(run: &LastRun) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let age = now.saturating_sub(run.at);
    let ago = match age {
        0..60 => format!("{age}s ago"),
        60..3600 => format!("{}m ago", age / 60),
        3600..86400 => format!("{}h ago", age / 3600),
        _ => format!("{}d ago", age / 86400),
    };
    let exit = run
        .exit
        .map_or("no exit".to_owned(), |c| format!("exit {c}"));
    format!("{exit} {} {}ms {ago}", run.result, run.duration_ms)
}

/// The `/hooks` lines; `selected` marks a row in the panel.
pub fn listing(
    rt: &Runtime,
    hooks: &[HookDef],
    selected: Option<usize>,
    target: Scope,
) -> Vec<String> {
    let options = rt
        .lock()
        .loaded
        .as_ref()
        .map(|l| l.options.clone())
        .unwrap_or_default();
    let flag = rt.lock().flag_trust_all;
    let mut lines = vec![format!(
        "Hooks  claude {}  codex {}  pi {}  scope {}  trustAll {}{}  config file {}",
        on_off(options.claude),
        on_off(options.codex),
        on_off(options.pi),
        ScopeOption::name(options.scope),
        on_off(options.trust_all),
        if flag { " (flag on)" } else { "" },
        target.name()
    )];
    if selected.is_some() {
        lines.push(HELP.to_owned());
    }
    if hooks.is_empty() {
        lines.push("No hooks found.".to_owned());
    }
    let mut event = "";
    for (index, def) in hooks.iter().enumerate() {
        if def.event != event {
            event = &def.event;
            lines.push(if selected.is_some() {
                style::bold(event)
            } else {
                event.to_owned()
            });
        }
        let marker = if selected == Some(index) { ">" } else { " " };
        let mut state = rt.trust_level(&def.hash).name().to_owned();
        if rt.is_disabled(&def.hash) {
            state.push_str(", disabled");
        }
        let last = rt
            .last_run(&def.hash)
            .map_or("never run".to_owned(), |run| run_text(&run));
        let matcher = def
            .matcher
            .as_deref()
            .filter(|m| !m.is_empty())
            .unwrap_or("*");
        let line = format!(
            " {marker} [{}] {matcher}  {}  ({state})  {last}",
            def.label(),
            def.summary()
        );
        lines.push(if selected == Some(index) {
            style::inverse(&line)
        } else {
            line
        });
    }
    lines
}
