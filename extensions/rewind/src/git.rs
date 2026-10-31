use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use pi_ext_sdk::{Error, Result, bail, info};
use serde::{Deserialize, Serialize};

use crate::gitcmd::{GIT_TIMEOUT, path_str, run_git};
use crate::nested;
use crate::stage::Staged;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FileChange {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// A snapshot commit, or the staging progress when the deadline passed first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taken {
    Commit(String),
    Partial { staged: usize, remaining: usize },
}

/// A bare repo under the extension state dir whose work tree is the session cwd.
#[derive(Debug, Clone)]
pub struct ShadowRepo {
    git_dir: PathBuf,
    work_tree: PathBuf,
    add_timeout: Duration,
}

impl ShadowRepo {
    pub fn new(storage: &Path, work_tree: &Path) -> Self {
        Self {
            git_dir: storage.join(".git"),
            work_tree: work_tree.to_path_buf(),
            add_timeout: GIT_TIMEOUT,
        }
    }

    /// How long git may take to store one batch of files.
    pub fn with_add_timeout(mut self, timeout: Duration) -> Self {
        self.add_timeout = timeout;
        self
    }

    pub fn add_timeout(&self) -> Duration {
        self.add_timeout
    }

    fn skip_file(&self) -> PathBuf {
        self.git_dir.join("info").join("pi-rewind-skip")
    }

    /// Leaves `path` out of later snapshots.
    pub(crate) fn skip_path(&self, path: &str) -> Result<()> {
        use std::io::Write;
        std::fs::create_dir_all(self.git_dir.join("info"))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.skip_file())?;
        writeln!(file, "{}", exclude_line(path))?;
        Ok(())
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
    }

    pub fn work_tree(&self) -> &Path {
        &self.work_tree
    }

    pub fn exists(&self) -> bool {
        self.git_dir.join("HEAD").is_file()
    }

    pub async fn ensure(&self) -> Result<()> {
        if !self.exists() {
            std::fs::create_dir_all(&self.git_dir)?;
            run_git(
                None,
                &["init", "--bare", "--quiet", path_str(&self.git_dir)?],
            )
            .await?;
        }
        self.configure().await
    }

    async fn configure(&self) -> Result<()> {
        let hooks = self.git_dir.join("pi-no-hooks");
        std::fs::create_dir_all(&hooks)?;
        let settings = [
            ("user.name", "pi-rewind"),
            ("user.email", "rewind@pi.invalid"),
            ("commit.gpgsign", "false"),
            ("core.autocrlf", "false"),
            ("core.hooksPath", path_str(&hooks)?),
            ("gc.auto", "0"),
            ("gc.pruneExpire", "never"),
        ];
        for (key, value) in settings {
            self.git(&["config", key, value]).await?;
        }
        Ok(())
    }

    /// Copies another session's checkpoint storage into this one.
    pub async fn clone_from(&self, source: &ShadowRepo) -> Result<()> {
        if let Some(parent) = self.git_dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let args = [
            "clone",
            "--bare",
            "--local",
            "--quiet",
            path_str(&source.git_dir)?,
            path_str(&self.git_dir)?,
        ];
        run_git(None, &args).await?;
        self.configure().await
    }

    pub(crate) fn full_args<'a>(&'a self, args: &[&'a str]) -> Result<Vec<&'a str>> {
        let mut full = vec![
            "--git-dir",
            path_str(&self.git_dir)?,
            "--work-tree",
            path_str(&self.work_tree)?,
        ];
        full.extend_from_slice(args);
        Ok(full)
    }

    pub async fn git(&self, args: &[&str]) -> Result<String> {
        run_git(Some(&self.work_tree), &self.full_args(args)?).await
    }

    pub async fn write_exclude(&self, patterns: &[String]) -> Result<()> {
        let mut lines = patterns.to_vec();
        lines.extend(nested::nested_repos(&self.work_tree, patterns));
        if let Ok(skipped) = std::fs::read_to_string(self.skip_file()) {
            lines.extend(skipped.lines().map(String::from));
        }
        if let Ok(inside) = self.git_dir.strip_prefix(&self.work_tree) {
            lines.push(format!("/{}/", inside.parent().unwrap_or(inside).display()));
        }
        let info = self.git_dir.join("info");
        std::fs::create_dir_all(&info)?;
        std::fs::write(info.join("exclude"), lines.join("\n") + "\n")?;
        Ok(())
    }

    /// Stages the whole work tree into the shadow index, skipping files over `max_bytes`.
    pub async fn stage_all(&self, patterns: &[String], max_bytes: Option<u64>) -> Result<()> {
        self.stage(patterns, max_bytes, None).await.map(drop)
    }

    /// Drops index entries that the excludes now ignore.
    pub(crate) async fn unstage_ignored(&self) -> Result<()> {
        let ignored = self
            .git(&["ls-files", "-z", "-i", "-c", "--exclude-standard"])
            .await?;
        let ignored: Vec<&str> = ignored.split('\0').filter(|p| !p.is_empty()).collect();
        for chunk in ignored.chunks(100) {
            let mut args = vec!["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"];
            args.extend_from_slice(chunk);
            self.git(&args).await?;
        }
        Ok(())
    }

    /// Removes git lock files of a killed git run. Callers hold the storage lock, so no
    /// other shadow git command is running.
    pub fn clear_stale_locks(&self) {
        let mut paths = vec![
            self.git_dir.join("index.lock"),
            self.git_dir.join("HEAD.lock"),
        ];
        if let Ok(entries) = std::fs::read_dir(self.git_dir.join("refs").join("heads")) {
            let refs = entries.flatten().map(|entry| entry.path());
            paths.extend(refs.filter(|path| path.extension().is_some_and(|ext| ext == "lock")));
        }
        for path in paths {
            if std::fs::remove_file(&path).is_ok() {
                info!("removed stale {}", path.display());
            }
        }
    }

    async fn head(&self) -> Option<String> {
        self.git(&["rev-parse", "--verify", "-q", "HEAD^{commit}"])
            .await
            .ok()
            .map(|out| out.trim().to_string())
            .filter(|hash| !hash.is_empty())
    }

    pub async fn has_commit(&self, commit: &str) -> bool {
        valid_ref(commit)
            && self
                .git(&["cat-file", "-e", &format!("{commit}^{{commit}}")])
                .await
                .is_ok()
    }

    /// Commits the staged index unless it equals `base`; returns the commit for this state.
    pub async fn commit_staged(&self, message: &str, base: Option<&str>) -> Result<String> {
        let base = match base {
            Some(base) if self.has_commit(base).await => Some(base.to_string()),
            _ => self.head().await,
        };
        if let Some(base) = &base
            && self
                .git(&["diff", "--cached", "--quiet", base])
                .await
                .is_ok()
        {
            return Ok(base.clone());
        }
        self.git(&[
            "commit",
            "-q",
            "--no-verify",
            "--allow-empty",
            "-m",
            message,
        ])
        .await?;
        self.head()
            .await
            .ok_or_else(|| Error::msg("checkpoint commit has no HEAD"))
    }

    pub async fn snapshot(
        &self,
        message: &str,
        base: Option<&str>,
        patterns: &[String],
        max_bytes: Option<u64>,
    ) -> Result<String> {
        self.stage_all(patterns, max_bytes).await?;
        self.commit_staged(message, base).await
    }

    /// [`Self::snapshot`] that commits only when staging finishes before `deadline`.
    pub async fn snapshot_within(
        &self,
        message: &str,
        base: Option<&str>,
        patterns: &[String],
        max_bytes: Option<u64>,
        deadline: Instant,
    ) -> Result<Taken> {
        match self.stage(patterns, max_bytes, Some(deadline)).await? {
            Staged::Complete => self.commit_staged(message, base).await.map(Taken::Commit),
            Staged::Partial { staged, remaining } => Ok(Taken::Partial { staged, remaining }),
        }
    }

    pub async fn numstat(&self, from: &str, to: &str) -> Result<Vec<FileChange>> {
        if from == to {
            return Ok(Vec::new());
        }
        let out = self.git(&["diff", "--numstat", from, to]).await?;
        Ok(parse_numstat(&out))
    }

    /// Writes `commit` into the work tree. Files outside the snapshot stay untouched.
    pub async fn checkout(&self, commit: &str) -> Result<()> {
        if !self.has_commit(commit).await {
            bail!("checkpoint commit {commit} is missing from storage");
        }
        self.git(&["reset", "-q", "--hard", commit]).await.map(drop)
    }
}

/// Paths `git add --ignore-errors` could not read, or `None` when it failed for another reason.
pub(crate) fn unreadable_files(stderr: &str) -> Option<Vec<String>> {
    let mut paths = Vec::new();
    for line in stderr
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(path) = line.strip_prefix("error: unable to index file ") {
            paths.push(path.trim_matches('\'').to_string());
        } else if !line.starts_with("error: open(") && !line.starts_with("warning: ") {
            return None;
        }
    }
    (!paths.is_empty()).then_some(paths)
}

pub fn parse_numstat(out: &str) -> Vec<FileChange> {
    out.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next()?.parse().unwrap_or(0);
            let removed = parts.next()?.parse().unwrap_or(0);
            let path = parts.next()?.trim();
            (!path.is_empty()).then(|| FileChange {
                path: path.to_string(),
                added,
                removed,
            })
        })
        .collect()
}

/// An anchored exclude pattern that matches only `path`.
fn exclude_line(path: &str) -> String {
    let mut line = String::from("/");
    for c in path.chars() {
        if matches!(c, '\\' | '*' | '?' | '[') {
            line.push('\\');
        }
        line.push(c);
    }
    if line.ends_with(' ') {
        line.insert(line.trim_end_matches(' ').len(), '\\');
    }
    line
}

fn valid_ref(commit: &str) -> bool {
    !commit.trim().is_empty() && !commit.contains(['\0', '\n', '\r']) && !commit.starts_with('-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_numstat_lines() {
        let changes = parse_numstat("3\t1\tsrc/a.rs\n-\t-\timg.png\n\nbad\n");
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "src/a.rs");
        assert_eq!((changes[0].added, changes[0].removed), (3, 1));
        assert_eq!((changes[1].added, changes[1].removed), (0, 0));
    }

    #[test]
    fn accepts_only_unreadable_file_errors() {
        let skipped = "error: open(\"secret.txt\"): Permission denied\nerror: unable to index file 'secret.txt'\n";
        assert_eq!(unreadable_files(skipped), Some(vec!["secret.txt".into()]));
        let locked = "fatal: Unable to create '/s/.git/index.lock': File exists.\n";
        assert_eq!(unreadable_files(locked), None);
        assert_eq!(unreadable_files(&format!("{skipped}{locked}")), None);
        assert_eq!(unreadable_files(""), None);
    }

    #[test]
    fn exclude_lines_match_one_literal_path() {
        assert_eq!(exclude_line("a/b.bin"), "/a/b.bin");
        assert_eq!(exclude_line("x*[1]?.iso"), "/x\\*\\[1]\\?.iso");
        assert_eq!(exclude_line("trail "), "/trail\\ ");
    }

    #[test]
    fn rejects_option_like_refs() {
        assert!(valid_ref("abc123"));
        assert!(!valid_ref("--all"));
        assert!(!valid_ref("a\nb"));
        assert!(!valid_ref(" "));
    }
}
