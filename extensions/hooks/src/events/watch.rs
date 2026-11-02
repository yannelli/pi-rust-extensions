use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use pi_ext_sdk::prelude::*;

use crate::decide::first_block;
use crate::dispatch::{Dispatch, dispatch};
use crate::matcher::Target;
use crate::model::Source;
use crate::paths::{env_file, project_root};
use crate::runtime::Runtime;

const POLL: Duration = Duration::from_secs(1);
const IDLE_AFTER: Duration = Duration::from_secs(60);
const SKILL_FILES_MAX: usize = 2000;

/// What the watcher saw last: existence, size, and mtime.
pub type Stamp = Option<(u64, Option<SystemTime>)>;

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.len(), meta.modified().ok()))
}

/// `add`, `change`, or `unlink` between two stamps.
pub fn change_kind(before: &Stamp, after: &Stamp) -> Option<&'static str> {
    match (before, after) {
        (None, Some(_)) => Some("add"),
        (Some(_), None) => Some("unlink"),
        (Some(a), Some(b)) if a != b => Some("change"),
        _ => None,
    }
}

/// Watcher state kept between polls.
#[derive(Default)]
pub struct Watch {
    pub files: HashMap<PathBuf, Stamp>,
    pub config: HashMap<PathBuf, Stamp>,
    pub idle_fired: bool,
}

/// FileChanged matchers name literal files in the cwd, split on `|`.
pub fn matcher_files(matchers: &[Option<String>], cwd: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = matchers
        .iter()
        .flatten()
        .flat_map(|m| m.split('|'))
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != "*")
        .map(|name| cwd.join(name))
        .collect();
    files.sort();
    files.dedup();
    files
}

/// Claude ConfigChange sources and their files.
pub fn config_files(rt: &Runtime, cwd: &Path) -> Vec<(&'static str, PathBuf)> {
    let root = project_root(cwd, ".claude");
    let claude = &rt.paths.claude_dir;
    let mut files = vec![
        ("user_settings", claude.join("settings.json")),
        ("project_settings", root.join(".claude/settings.json")),
        ("local_settings", root.join(".claude/settings.local.json")),
    ];
    for dir in policy_dirs() {
        files.push(("policy_settings", dir.join("managed-settings.json")));
        files.extend(
            list_files(&dir.join("managed-settings.d"), 100)
                .into_iter()
                .map(|f| ("policy_settings", f)),
        );
    }
    for skills in [claude.join("skills"), root.join(".claude/skills")] {
        files.extend(
            list_files(&skills, SKILL_FILES_MAX)
                .into_iter()
                .map(|f| ("skills", f)),
        );
    }
    files
}

fn policy_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        vec![PathBuf::from("/Library/Application Support/ClaudeCode")]
    } else if cfg!(windows) {
        vec![PathBuf::from(r"C:\Program Files\ClaudeCode")]
    } else {
        vec![PathBuf::from("/etc/claude-code")]
    }
}

fn list_files(dir: &Path, max: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            match entry.file_type() {
                Ok(kind) if kind.is_dir() => stack.push(path),
                Ok(_) => out.push(path),
                Err(_) => {}
            }
            if out.len() >= max {
                return out;
            }
        }
    }
    out
}

/// Polls watched files, config files, the cwd, and idle time while the extension runs.
pub async fn run(rt: Arc<Runtime>, host: Host) {
    let mut watch = Watch::default();
    let mut primed = false;
    loop {
        tokio::time::sleep(POLL).await;
        let Some(snap) = host.snapshot().map(|s| (*s).clone()) else {
            continue;
        };
        if rt.lock().loaded.is_none() {
            continue;
        }
        poll(&rt, &host, &snap, &mut watch, primed).await;
        primed = true;
    }
}

async fn poll(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, watch: &mut Watch, primed: bool) {
    let cwd = PathBuf::from(&snap.cwd);
    let matchers: Vec<Option<String>> = rt
        .hooks()
        .into_iter()
        .filter(|def| def.event == "FileChanged" && def.source != Source::Codex)
        .map(|def| def.matcher)
        .collect();
    let mut files = matcher_files(&matchers, &cwd);
    files.extend(rt.lock().watch_paths.iter().cloned());
    for file in files {
        let now = stamp(&file);
        let before = watch.files.insert(file.clone(), now);
        let kind = before.and_then(|before| change_kind(&before, &now));
        if let Some(kind) = kind.filter(|_| primed) {
            file_changed(rt, host, snap, &file, kind).await;
        }
    }
    let mut changed = Vec::new();
    for (source, file) in config_files(rt, &cwd) {
        let now = stamp(&file);
        let before = watch.config.insert(file.clone(), now);
        let moved = match &before {
            Some(before) => change_kind(before, &now).is_some(),
            None => primed && now.is_some(),
        };
        if moved && primed {
            changed.push((source, file));
        }
    }
    if !changed.is_empty() {
        config_changed(rt, host, snap, changed).await;
    }
    idle(rt, host, snap, watch).await;
}

pub async fn cwd_changed(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, old: &str) {
    let file = env_file(
        &rt.agent_dir(host),
        snap.session_id.as_deref().unwrap_or_default(),
    );
    if file.exists() {
        let _ = std::fs::write(&file, "");
    }
    let payload = json!({ "old_cwd": old, "new_cwd": snap.cwd });
    let d = Dispatch::new("CwdChanged", Target::Any, payload).claude_only();
    let outcomes = dispatch(rt, host, snap, d).await;
    apply_watch_paths(rt, &outcomes);
}

async fn file_changed(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, file: &Path, kind: &str) {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let payload = json!({ "file_path": file.display().to_string(), "event": kind });
    let d = Dispatch::new("FileChanged", Target::Value(name), payload).claude_only();
    let outcomes = dispatch(rt, host, snap, d).await;
    apply_watch_paths(rt, &outcomes);
}

/// ConfigChange per changed file; the new hooks config loads only when no hook blocks it.
async fn config_changed(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    changed: Vec<(&'static str, PathBuf)>,
) {
    let mut blocked = false;
    for (source, file) in changed {
        let payload = json!({ "source": source, "file_path": file.display().to_string() });
        let d = Dispatch::new("ConfigChange", Target::Value(source.into()), payload).claude_only();
        let outcomes = dispatch(rt, host, snap, d).await;
        blocked |= source != "policy_settings" && first_block(&outcomes).is_some();
    }
    if blocked {
        pi_ext_sdk::info!(
            "hooks: a ConfigChange hook blocked the change; keeping the loaded hooks"
        );
    } else {
        rt.reload(host, snap).await;
    }
}

/// Notification `idle_prompt`: 60 seconds after the agent settled without new input.
async fn idle(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot, watch: &mut Watch) {
    let settled = rt.lock().settled_at;
    let Some(at) = settled else {
        watch.idle_fired = false;
        return;
    };
    if watch.idle_fired || at.elapsed() < IDLE_AFTER || !snap.has_ui {
        return;
    }
    watch.idle_fired = true;
    notify(
        rt,
        host,
        snap,
        "idle_prompt",
        "pi is waiting for your input",
        None,
    )
    .await;
}

pub async fn notify(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    kind: &str,
    message: &str,
    title: Option<&str>,
) {
    let payload =
        json!({ "message": message, "title": title.unwrap_or("pi"), "notification_type": kind });
    let d = Dispatch::new("Notification", Target::Value(kind.into()), payload).claude_only();
    dispatch(rt, host, snap, d).await;
}

/// `watchPaths` from CwdChanged, FileChanged, or SessionStart replaces the dynamic list.
pub fn apply_watch_paths(rt: &Runtime, outcomes: &[crate::output::Outcome]) {
    if let Some(paths) = outcomes.iter().rev().find_map(|o| o.watch_paths.clone()) {
        rt.lock().watch_paths = paths.into_iter().map(PathBuf::from).collect();
    }
}

/// Starts the watcher once per extension process.
pub fn start(rt: Arc<Runtime>, host: Host) {
    let started = Instant::now();
    tokio::spawn(async move {
        run(rt, host).await;
        pi_ext_sdk::info!("hooks: watcher stopped after {:?}", started.elapsed());
    });
}

/// `/add-dir <path>`: records a working directory and fires DirectoryAdded (`slash_command`).
pub async fn add_dir(rt: Arc<Runtime>, args: String, ctx: CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    let arg = args.trim();
    if arg.is_empty() {
        return host.notify_warning("Usage: /add-dir <path>").await;
    }
    let dir = PathBuf::from(crate::tools::absolute_path(arg, Path::new(&snap.cwd)));
    if !dir.is_dir() {
        return host
            .notify_warning(format!("Not a directory: {}", dir.display()))
            .await;
    }
    let known = std::iter::once(PathBuf::from(&snap.cwd)).chain(rt.lock().added_dirs.clone());
    if known.into_iter().any(|d| dir.starts_with(&d)) {
        return host
            .notify_info(format!("{} is already a working directory", dir.display()))
            .await;
    }
    rt.lock().added_dirs.push(dir.clone());
    let path = dir.display().to_string();
    let note = format!("The user added a working directory: {path}");
    let message = CustomMessage::new("hooks-context", note)
        .display(false)
        .deliver_as(DeliverAs::NextTurn);
    host.send_message(message).await?;
    host.notify_info(format!("Added working directory {path}"))
        .await?;
    let (rt, host, snap) = (rt.clone(), host.clone(), snap.clone());
    tokio::spawn(async move {
        let payload = json!({ "directory": path, "source": "slash_command" });
        let d = Dispatch::new(
            "DirectoryAdded",
            Target::Value("slash_command".into()),
            payload,
        )
        .claude_only();
        let outcomes = dispatch(&rt, &host, &snap, d).await;
        let notes: Vec<String> = outcomes
            .iter()
            .filter_map(|o| o.system_message.clone())
            .collect();
        if !notes.is_empty() {
            rt.lock().held_context.extend(notes);
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps_give_change_kinds() {
        let a = Some((1, None));
        let b = Some((2, None));
        assert_eq!(change_kind(&None, &a), Some("add"));
        assert_eq!(change_kind(&a, &None), Some("unlink"));
        assert_eq!(change_kind(&a, &b), Some("change"));
        assert_eq!(change_kind(&a, &a), None);
    }

    #[test]
    fn matchers_name_files_in_the_cwd() {
        let cwd = Path::new("/r");
        let files = matcher_files(&[Some(".envrc|.env".into()), None, Some("*".into())], cwd);
        assert_eq!(
            files,
            [PathBuf::from("/r/.env"), PathBuf::from("/r/.envrc")]
        );
    }
}
