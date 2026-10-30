use std::collections::HashSet;
#[cfg(unix)]
use std::process::Stdio;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// Process groups of one extension instance that are still running.
#[derive(Clone, Default)]
pub struct Live(Arc<Mutex<HashSet<u32>>>);

impl Live {
    fn set(&self) -> MutexGuard<'_, HashSet<u32>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Kills every group that is still running. The `on_shutdown` hook calls this,
    /// because the runtime can exit before it drops in-flight handlers.
    pub fn kill_all(&self) -> usize {
        let pids: Vec<u32> = self.set().drain().collect();
        pids.iter().copied().for_each(signal);
        pids.len()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.set().len()
    }
}

/// Kills a child's process group, so a timeout or cancel also stops its descendants.
/// Call `disarm` after a normal exit: the reaped leader PID must not be signalled.
pub struct Group(Option<u32>, Live);

impl Group {
    pub fn new(pid: Option<u32>, live: &Live) -> Self {
        if let Some(pid) = pid {
            live.set().insert(pid);
        }
        Self(pid, live.clone())
    }

    pub fn disarm(&mut self) {
        if let Some(pid) = self.0.take() {
            self.1.set().remove(&pid);
        }
    }

    pub fn kill(&mut self) {
        if let Some(pid) = self.0.take() {
            self.1.set().remove(&pid);
            signal(pid);
        }
    }

    /// `kill` without blocking a runtime thread on the `kill` command.
    pub async fn stop(&mut self) {
        let Some(pid) = self.0.take() else {
            return;
        };
        self.1.set().remove(&pid);
        #[cfg(unix)]
        {
            let status = tokio::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            if tokio::time::timeout(std::time::Duration::from_secs(2), status)
                .await
                .is_err()
            {
                signal(pid);
            }
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(unix)]
fn signal(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-KILL", "--", &format!("-{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

#[cfg(not(unix))]
fn signal(_pid: u32) {}
