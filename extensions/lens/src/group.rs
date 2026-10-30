use std::process::Stdio;

/// Kills a child's process group, so a timeout or cancel also stops its descendants.
/// Call `disarm` after a normal exit: the reaped leader PID must not be signalled.
pub struct Group(Option<u32>);

impl Group {
    pub fn new(pid: Option<u32>) -> Self {
        Self(pid)
    }

    pub fn disarm(&mut self) {
        self.0 = None;
    }

    pub fn kill(&mut self) {
        #[cfg(unix)]
        if let Some(pid) = self.0.take() {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", "--", &format!("-{pid}")])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.kill();
    }
}
