//! The GitHub repo of the cwd, from the URL of its `origin` remote.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

/// `owner/name` of an `https`, `ssh`, `git`, or scp-style (`git@host:owner/name.git`) URL.
pub fn parse_remote(url: &str) -> Option<String> {
    let url = url.trim();
    let path = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/')?.1,
        None => {
            let (host, path) = url.split_once(':')?;
            if host.contains('/') || path.starts_with("//") {
                return None;
            }
            path
        }
    };
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let path = path.trim_end_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    let parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    match parts.as_slice() {
        [.., owner, name] => Some(format!("{owner}/{name}")),
        _ => None,
    }
}

/// `owner/name` of the `origin` remote of the repo that holds `cwd`.
pub async fn detect(cwd: &Path) -> Option<String> {
    let mut command = tokio::process::Command::new("git");
    command
        .arg("-C")
        .arg(cwd)
        .args(["remote", "get-url", "origin"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(5), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_remote(&String::from_utf8_lossy(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_ssh_and_scp_urls() {
        let want = Some("yannelli/pi-rust".to_owned());
        for url in [
            "https://github.com/yannelli/pi-rust",
            "https://github.com/yannelli/pi-rust.git",
            "https://user:token@github.com/yannelli/pi-rust/\n",
            "http://github.com/yannelli/pi-rust.git",
            "ssh://git@github.com/yannelli/pi-rust.git",
            "ssh://git@ssh.github.com:443/yannelli/pi-rust.git",
            "git://github.com/yannelli/pi-rust",
            "git@github.com:yannelli/pi-rust.git",
            "git@github-work:yannelli/pi-rust",
        ] {
            assert_eq!(parse_remote(url), want, "{url}");
        }
        assert_eq!(
            parse_remote("git@github.com:Yannelli/Pi-Rust.git").as_deref(),
            Some("Yannelli/Pi-Rust")
        );
        assert_eq!(parse_remote("https://github.com/yannelli"), None);
        assert_eq!(parse_remote("/srv/git/repo"), None);
        assert_eq!(parse_remote(""), None);
    }

    #[tokio::test]
    async fn detect_reads_origin_and_skips_other_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .output()
                .map(|o| o.status.success())
        };
        if git(&["init", "-q"]).ok() != Some(true) {
            eprintln!("skipping: git is not available");
            return;
        }
        assert_eq!(detect(dir.path()).await, None);
        git(&["remote", "add", "origin", "git@github.com:Owner/Repo.git"]).unwrap();
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert_eq!(detect(&sub).await.as_deref(), Some("Owner/Repo"));
    }
}
