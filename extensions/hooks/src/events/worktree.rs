//! `/worktree`: WorktreeCreate and WorktreeRemove around a pi session in a separate working copy.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use pi_ext_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::dispatch::{Dispatch, dispatch};
use crate::matcher::Target;
use crate::model::trust_hash;
use crate::paths::state_dir;
use crate::runtime::Runtime;
use crate::tools::absolute_path;

/// A worktree this extension created, kept on disk because a session switch restarts it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub name: String,
    pub path: String,
    pub origin: String,
    pub by_hook: bool,
}

fn registry_file(agent_dir: &Path) -> PathBuf {
    state_dir(agent_dir).join("worktrees.json")
}

pub fn load(agent_dir: &Path) -> Vec<Worktree> {
    std::fs::read_to_string(registry_file(agent_dir))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(agent_dir: &Path, list: &[Worktree]) {
    let file = registry_file(agent_dir);
    let text = serde_json::to_string_pretty(list).unwrap_or_default();
    if let Err(err) = file
        .parent()
        .map(std::fs::create_dir_all)
        .transpose()
        .and_then(|_| std::fs::write(&file, text))
    {
        pi_ext_sdk::warn!("hooks: cannot write {}: {err}", file.display());
    }
}

fn now() -> (u128, String) {
    let ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    (ms, iso_time(ms))
}

/// `2026-11-02T18:21:47.708Z` from Unix milliseconds.
pub fn iso_time(ms: u128) -> String {
    let secs = (ms / 1000) as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (y, m, d) = civil(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        ms % 1000
    )
}

fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// pi's session folder for `cwd`: `<agent>/sessions/--<cwd with separators as ->--`.
pub fn session_dir(agent_dir: &Path, cwd: &str) -> PathBuf {
    let trimmed = cwd.strip_prefix(['/', '\\']).unwrap_or(cwd);
    agent_dir
        .join("sessions")
        .join(format!("--{}--", trimmed.replace(['/', '\\', ':'], "-")))
}

/// Writes an empty session whose cwd is `cwd` and returns its file.
pub fn new_session_file(agent_dir: &Path, cwd: &str) -> std::io::Result<PathBuf> {
    let (ms, time) = now();
    let hex = trust_hash(cwd, &time, None, &json!(ms));
    let id = format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    );
    let dir = session_dir(agent_dir, cwd);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{}_{id}.jsonl", time.replace([':', '.'], "-")));
    let header =
        json!({ "type": "session", "version": 3, "id": id, "timestamp": time, "cwd": cwd });
    std::fs::write(&file, format!("{header}\n"))?;
    Ok(file)
}

fn slug() -> String {
    let (ms, _) = now();
    format!(
        "wt-{}",
        &trust_hash("worktree", &ms.to_string(), None, &json!(null))[..6]
    )
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    cwd.ancestors()
        .find(|d| d.join(".git").exists())
        .map(Path::to_path_buf)
}

async fn git(dir: &Path, args: &[&str]) -> std::result::Result<String, String> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await;
    match output {
        Ok(out) if out.status.success() => Ok(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(out) => Err(String::from_utf8_lossy(&out.stderr).trim().to_owned()),
        Err(err) => Err(err.to_string()),
    }
}

/// WorktreeCreate hooks replace `git worktree add`; their path must be a directory.
async fn create(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    name: &str,
) -> std::result::Result<Worktree, String> {
    let cwd = PathBuf::from(&snap.cwd);
    let hooked = rt
        .hooks()
        .iter()
        .any(|d| d.event == "WorktreeCreate" && !d.source.is_codex());
    if hooked {
        let d = Dispatch::new("WorktreeCreate", Target::Any, json!({ "name": name })).claude_only();
        let outcomes = dispatch(rt, host, snap, d).await;
        if let Some(error) = outcomes.iter().find_map(|o| o.error.clone()) {
            return Err(format!("WorktreeCreate hook failed: {error}"));
        }
        let path = outcomes
            .iter()
            .find_map(|o| o.worktree_path.clone())
            .ok_or("WorktreeCreate hook returned no path")?;
        let path = absolute_path(&path, &cwd);
        if !Path::new(&path).is_dir() {
            return Err(format!("WorktreeCreate path is not a directory: {path}"));
        }
        return Ok(Worktree {
            name: name.into(),
            path,
            origin: snap.cwd.clone(),
            by_hook: true,
        });
    }
    let root = git_root(&cwd)
        .ok_or("not in a git repository; add a WorktreeCreate hook for other version control")?;
    let path = root.join(".pi").join("worktrees").join(name);
    let target = path.display().to_string();
    git(&root, &["worktree", "add", "-b", name, &target]).await?;
    Ok(Worktree {
        name: name.into(),
        path: target,
        origin: snap.cwd.clone(),
        by_hook: false,
    })
}

/// WorktreeRemove for hook worktrees; `git worktree remove` for the others.
async fn remove(
    rt: &Arc<Runtime>,
    host: &Host,
    snap: &Snapshot,
    wt: &Worktree,
    force: bool,
) -> std::result::Result<(), String> {
    let hooked = rt
        .hooks()
        .iter()
        .any(|d| d.event == "WorktreeRemove" && !d.source.is_codex());
    if hooked {
        let d = Dispatch::new(
            "WorktreeRemove",
            Target::Any,
            json!({ "worktree_path": wt.path }),
        )
        .claude_only();
        let outcomes = dispatch(rt, host, snap, d).await;
        let failed = outcomes.iter().find_map(|o| o.error.clone());
        return match failed {
            Some(error) if Path::new(&wt.path).exists() => {
                Err(format!("WorktreeRemove hook failed: {error}"))
            }
            _ => Ok(()),
        };
    }
    let origin = git_root(Path::new(&wt.origin)).unwrap_or_else(|| PathBuf::from(&wt.origin));
    let mut args = vec!["worktree", "remove"];
    if force || wt.by_hook {
        args.push("--force");
    }
    args.push(&wt.path);
    git(&origin, &args).await.map(drop)
}

/// True when git sees no changed or untracked files in `path`.
async fn is_clean(path: &str) -> bool {
    git(Path::new(path), &["status", "--porcelain"])
        .await
        .is_ok_and(|out| out.trim().is_empty())
}

async fn switch_to(host: &Host, agent_dir: &Path, cwd: &str) -> Result<()> {
    let file = new_session_file(agent_dir, cwd)?;
    host.pi_call(
        "ctx",
        "switchSession",
        vec![json!(file.display().to_string())],
    )
    .await?;
    Ok(())
}

/// `/worktree [name]` enters a new worktree; `/worktree exit [--force]` removes it and returns.
pub async fn command(rt: Arc<Runtime>, args: String, ctx: CommandCtx) -> Result<()> {
    let (host, snap) = (ctx.host(), ctx.snapshot());
    rt.ensure_loaded(host, snap).await;
    let agent_dir = rt.agent_dir(host);
    let mut words = args.split_whitespace();
    let first = words.next();
    if first == Some("exit") {
        let force = words.any(|w| w == "--force");
        let mut list = load(&agent_dir);
        let Some(index) = list.iter().position(|w| w.path == snap.cwd) else {
            return host
                .notify_warning("This session is not in a worktree that /worktree created")
                .await;
        };
        let wt = list[index].clone();
        if !force && !wt.by_hook && !is_clean(&wt.path).await {
            return host
                .notify_warning("The worktree has changes; use /worktree exit --force to remove it")
                .await;
        }
        switch_to(host, &agent_dir, &wt.origin).await?;
        if let Err(error) = remove(&rt, host, snap, &wt, force).await {
            return host
                .notify_warning(format!("Kept {}: {error}", wt.path))
                .await;
        }
        list.remove(index);
        save(&agent_dir, &list);
        return host
            .notify_info(format!("Removed worktree {}", wt.path))
            .await;
    }
    let name = first.map_or_else(slug, str::to_owned);
    match create(&rt, host, snap, &name).await {
        Ok(wt) => {
            let mut list = load(&agent_dir);
            list.push(wt.clone());
            save(&agent_dir, &list);
            switch_to(host, &agent_dir, &wt.path).await
        }
        Err(error) => {
            host.notify_error(format!("Cannot create worktree {name}: {error}"))
                .await
        }
    }
}

/// `--worktree [name]` at startup: create the worktree and queue a switch into it.
pub async fn on_startup(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot) {
    let name = match snap.flag(WORKTREE_FLAG) {
        Some(Value::String(name)) if !name.trim().is_empty() => name.trim().to_owned(),
        Some(Value::Bool(true)) => slug(),
        _ => return,
    };
    let agent_dir = rt.agent_dir(host);
    match create(rt, host, snap, &name).await {
        Ok(wt) => {
            let mut list = load(&agent_dir);
            list.push(wt.clone());
            save(&agent_dir, &list);
            if let Err(err) = switch_to(host, &agent_dir, &wt.path).await {
                let _ = host
                    .notify_error(format!(
                        "Created {} but cannot switch to it: {err}",
                        wt.path
                    ))
                    .await;
            }
        }
        Err(error) => {
            let _ = host
                .notify_error(format!("Cannot create worktree {name}: {error}"))
                .await;
        }
    }
}

/// Extension flag that starts the session in a new worktree, like `claude --worktree`.
pub const WORKTREE_FLAG: &str = "worktree";

/// On quit, a clean worktree from `/worktree` is removed, as Claude does for unnamed sessions.
pub async fn on_quit(rt: &Arc<Runtime>, host: &Host, snap: &Snapshot) {
    let agent_dir = rt.agent_dir(host);
    let mut list = load(&agent_dir);
    let Some(index) = list.iter().position(|w| w.path == snap.cwd) else {
        return;
    };
    let wt = list[index].clone();
    if !wt.by_hook && !is_clean(&wt.path).await {
        return;
    }
    if remove(rt, host, snap, &wt, false).await.is_ok() {
        list.remove(index);
        save(&agent_dir, &list);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_and_session_dirs() {
        assert_eq!(iso_time(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso_time(1_793_644_907_708), "2026-11-02T18:41:47.708Z");
        let dir = session_dir(Path::new("/a"), "/home/u/repo");
        assert_eq!(dir, PathBuf::from("/a/sessions/--home-u-repo--"));
    }

    #[test]
    fn session_file_has_a_header() {
        let dir = tempfile::tempdir().unwrap();
        let file = new_session_file(dir.path(), "/w/x").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        let header: Value = serde_json::from_str(text.trim()).unwrap();
        assert_eq!(header["cwd"], "/w/x");
        assert_eq!(header["type"], "session");
        assert!(file.starts_with(dir.path().join("sessions/--w-x--")));
    }
}
