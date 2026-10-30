use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::process;
use crate::task::{self, Status, Task, default_name, resolve_in};

const KILL_GRACE: Duration = Duration::from_secs(3);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    pub name: Option<String>,
    pub command: String,
    pub description: Option<String>,
    pub is_agent: bool,
    pub timeout_seconds: Option<u64>,
    pub notify_on_completion: bool,
    pub trigger_on_completion: bool,
    pub survive_reload: bool,
}

#[derive(Default)]
pub struct Registry {
    inner: Mutex<Inner>,
    pub monitor_started: AtomicBool,
}

#[derive(Default)]
struct Inner {
    dir: Option<PathBuf>,
    tasks: Vec<Task>,
}

impl Registry {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Binds the registry to a session directory once and loads the tasks stored there.
    pub fn bind(&self, dir: PathBuf) {
        let mut inner = self.lock();
        if inner.dir.is_none() {
            inner.tasks = task::load_all(&dir);
            inner.dir = Some(dir);
        }
    }

    pub fn is_bound(&self) -> bool {
        self.lock().dir.is_some()
    }

    pub fn tasks(&self) -> Vec<Task> {
        self.lock().tasks.clone()
    }

    pub fn running(&self) -> Vec<Task> {
        self.lock()
            .tasks
            .iter()
            .filter(|t| t.status.is_running())
            .cloned()
            .collect()
    }

    pub fn start(&self, options: StartOptions, cwd: &str) -> Result<Task, String> {
        let base = self.lock().dir.clone().ok_or("no session directory yet")?;
        let id = task::new_id();
        let dir = base.join(&id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let name = options
            .name
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| default_name(&options.command));
        let mut task = Task {
            id,
            name,
            command: options.command,
            description: options.description,
            cwd: cwd.to_owned(),
            is_agent: options.is_agent,
            pid: None,
            dir,
            started_at: task::now_ms(),
            ended_at: None,
            timeout_seconds: options.timeout_seconds.filter(|s| *s > 0),
            notify_on_completion: options.notify_on_completion,
            trigger_on_completion: options.notify_on_completion && options.trigger_on_completion,
            survive_reload: options.survive_reload,
            status: Status::Running,
            exit_code: None,
            notified: false,
            error: None,
        };
        let spawned = process::spawn(
            &task.command,
            Path::new(cwd),
            &task.output_path(),
            &task.exit_path(),
        );
        match spawned {
            Ok(pid) => task.pid = Some(pid),
            Err(err) => {
                let _ = std::fs::remove_dir_all(&task.dir);
                return Err(format!("cannot start `{}`: {err}", task.command));
            }
        }
        task.save()
            .map_err(|e| format!("cannot save task state: {e}"))?;
        self.lock().tasks.push(task.clone());
        Ok(task)
    }

    /// Finds a task by id or by an unambiguous id prefix.
    pub fn resolve(&self, query: &str) -> Result<Task, String> {
        resolve_in(&self.lock().tasks, query).cloned()
    }

    /// Applies `update` to the task `id` and saves it; returns the updated task.
    fn update(&self, id: &str, update: impl FnOnce(&mut Task) -> bool) -> Option<Task> {
        let mut inner = self.lock();
        let task = inner.tasks.iter_mut().find(|t| t.id == id)?;
        if !update(task) {
            return None;
        }
        if let Err(err) = task.save() {
            pi_ext_sdk::warn!("cannot save task {}: {err}", task.id);
        }
        Some(task.clone())
    }

    /// Stops a running task: TERM to its process group, KILL after a grace period.
    pub async fn stop(&self, query: &str, reason: &str) -> Result<Task, String> {
        let id = self.resolve(query)?.id;
        let task = self
            .update(&id, |t| {
                let stopped = t.finish(Status::Killed, None);
                if stopped {
                    t.notified = true;
                    t.error = Some(reason.to_owned());
                }
                stopped
            })
            .ok_or_else(|| format!("task {id} is not running"))?;
        let Some(pid) = task.pid else {
            return Ok(task);
        };
        if let Err(err) = process::terminate(pid, KILL_GRACE).await {
            self.update(&id, |t| {
                t.error = Some(err.clone());
                true
            });
            return Err(err);
        }
        Ok(task)
    }

    /// Checks running tasks once and returns the finished tasks that still owe a notification.
    pub async fn poll(&self, check_alive: bool) -> Vec<Task> {
        let now = task::now_ms();
        for task in self.running() {
            if let Some(code) = task.read_exit_code() {
                let status = if code == 0 {
                    Status::Completed
                } else {
                    Status::Failed
                };
                self.update(&task.id, |t| t.finish(status, Some(code)));
            } else if task.timed_out(now) {
                let updated = self.update(&task.id, |t| {
                    let done = t.finish(Status::TimedOut, None);
                    t.error = Some(format!(
                        "timed out after {}s",
                        t.timeout_seconds.unwrap_or(0)
                    ));
                    done
                });
                if let (Some(_), Some(pid)) = (updated, task.pid) {
                    tokio::spawn(async move {
                        if let Err(err) = process::terminate(pid, KILL_GRACE).await {
                            pi_ext_sdk::warn!("{err}");
                        }
                    });
                }
            } else if check_alive && !process::is_alive(task.pid).await {
                let code = task.read_exit_code();
                self.update(&task.id, |t| match code {
                    Some(0) => t.finish(Status::Completed, Some(0)),
                    Some(c) => t.finish(Status::Failed, Some(c)),
                    None => {
                        t.error = Some("process ended without an exit status".into());
                        t.finish(Status::Lost, None)
                    }
                });
            }
        }
        self.lock()
            .tasks
            .iter()
            .filter(|t| !t.status.is_running() && t.notify_on_completion && !t.notified)
            .cloned()
            .collect()
    }

    pub fn mark_notified(&self, id: &str) {
        self.update(id, |t| {
            t.notified = true;
            true
        });
    }

    /// Removes finished tasks and their output; returns how many it removed.
    pub fn clear_finished(&self) -> usize {
        let mut inner = self.lock();
        let before = inner.tasks.len();
        inner.tasks.retain(|t| {
            let keep = t.status.is_running();
            if !keep {
                let _ = std::fs::remove_dir_all(&t.dir);
            }
            keep
        });
        before - inner.tasks.len()
    }

    /// `session_shutdown`: on reload keep tasks that survive reloads, else stop all running tasks.
    pub async fn shutdown(&self, reason: &str) -> usize {
        let reload = reason == "reload";
        let victims: Vec<Task> = self
            .running()
            .into_iter()
            .filter(|t| !(reload && t.survive_reload))
            .filter_map(|t| {
                self.update(&t.id, |t| {
                    t.notified = true;
                    t.error = Some(format!("stopped on session {reason}"));
                    t.finish(Status::Killed, None)
                })
            })
            .collect();
        let mut stops = tokio::task::JoinSet::new();
        for pid in victims.iter().filter_map(|t| t.pid) {
            stops.spawn(process::terminate(pid, SHUTDOWN_GRACE));
        }
        while let Some(result) = stops.join_next().await {
            if let Ok(Err(err)) = result {
                pi_ext_sdk::warn!("{err}");
            }
        }
        victims.len()
    }

    pub fn monitor_once(&self) -> bool {
        !self.monitor_started.swap(true, Ordering::SeqCst)
    }
}
