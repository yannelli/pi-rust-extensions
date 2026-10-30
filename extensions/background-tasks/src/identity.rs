//! Process identity checks that keep signals away from reused pids.

use std::io;
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// `pid` is the process started for the task.
    Same,
    Gone,
    /// `pid` belongs to another process, or its identity cannot be confirmed.
    Other,
}

/// Compares `pid` with the start time recorded at spawn. A pid without a recorded or readable
/// start time counts as `Other`.
pub fn owner(pid: u32, expected: Option<&str>) -> Owner {
    match identity(pid) {
        Ok(Some(current)) if expected == Some(current.as_str()) => Owner::Same,
        Ok(None) => Owner::Gone,
        _ => Owner::Other,
    }
}

/// True when group `pgid` still holds a process from `members`. The system does not give a
/// pgid to a new process while its group exists.
pub fn group_unchanged(pgid: u32, members: &[(u32, String)]) -> bool {
    group_members(pgid).is_ok_and(|now| now.iter().any(|member| members.contains(member)))
}

fn normalize(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(unix)]
fn ps(args: &[&str]) -> io::Result<(bool, String)> {
    let out = Command::new("ps")
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    ))
}

/// The start time `ps` reports for `pid`; `Ok(None)` when no such process exists.
#[cfg(unix)]
pub fn identity(pid: u32) -> io::Result<Option<String>> {
    let (ok, out) = ps(&["-o", "lstart=", "-p", &pid.to_string()])?;
    let text = normalize(&out);
    Ok((ok && !text.is_empty()).then_some(text))
}

/// `(pid, start time)` of each process in group `pgid`.
#[cfg(unix)]
pub fn group_members(pgid: u32) -> io::Result<Vec<(u32, String)>> {
    let (ok, out) = ps(&["-A", "-o", "pid=,pgid=,lstart="])?;
    if !ok {
        return Err(io::Error::other("ps failed"));
    }
    Ok(out
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let pid = parts.next()?.parse().ok()?;
            let group: u32 = parts.next()?.parse().ok()?;
            (group == pgid).then(|| (pid, normalize(&parts.collect::<Vec<_>>().join(" "))))
        })
        .collect())
}

#[cfg(windows)]
pub fn identity(pid: u32) -> io::Result<Option<String>> {
    let script =
        format!("(Get-Process -Id {pid} -ErrorAction SilentlyContinue).StartTime.ToFileTimeUtc()");
    let out = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    let text = normalize(&String::from_utf8_lossy(&out.stdout));
    Ok((!text.is_empty()).then_some(text))
}

#[cfg(windows)]
pub fn group_members(_pgid: u32) -> io::Result<Vec<(u32, String)>> {
    Ok(Vec::new())
}
