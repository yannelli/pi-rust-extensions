use std::path::{Path, PathBuf};

use pi_ext_sdk::{Error, Result, bail};
use serde::{Deserialize, Serialize};

use crate::gitcmd::{path_str, run_git};
use crate::nested;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FileChange {
    pub path: String,
    pub added: u64,
    pub removed: u64,
}

/// A bare repo under the extension state dir whose work tree is the session cwd.
#[derive(Debug, Clone)]
pub struct ShadowRepo {
    git_dir: PathBuf,
    work_tree: PathBuf,
}

impl ShadowRepo {
    pub fn new(storage: &Path, work_tree: &Path) -> Self {
        Self {
            git_dir: storage.join(".git"),
            work_tree: work_tree.to_path_buf(),
        }
    }

    pub fn git_dir(&self) -> &Path {
        &self.git_dir
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

    pub async fn git(&self, args: &[&str]) -> Result<String> {
        let mut full = vec![
            "--git-dir",
            path_str(&self.git_dir)?,
            "--work-tree",
            path_str(&self.work_tree)?,
        ];
        full.extend_from_slice(args);
        run_git(Some(&self.work_tree), &full).await
    }

    pub async fn write_exclude(&self, patterns: &[String]) -> Result<()> {
        let mut lines = patterns.to_vec();
        lines.extend(nested::nested_repos(&self.work_tree, patterns));
        if let Ok(inside) = self.git_dir.strip_prefix(&self.work_tree) {
            lines.push(format!("/{}/", inside.parent().unwrap_or(inside).display()));
        }
        let info = self.git_dir.join("info");
        std::fs::create_dir_all(&info)?;
        std::fs::write(info.join("exclude"), lines.join("\n") + "\n")?;
        Ok(())
    }

    /// Stages the work tree into the shadow index, skipping files over `max_bytes`.
    pub async fn stage_all(&self, patterns: &[String], max_bytes: Option<u64>) -> Result<()> {
        self.write_exclude(patterns).await?;
        let ignored = self
            .git(&["ls-files", "-z", "-i", "-c", "--exclude-standard"])
            .await?;
        let ignored: Vec<&str> = ignored.split('\0').filter(|p| !p.is_empty()).collect();
        for chunk in ignored.chunks(100) {
            let mut args = vec!["rm", "--cached", "-r", "-q", "--ignore-unmatch", "--"];
            args.extend_from_slice(chunk);
            self.git(&args).await?;
        }
        let oversized = self.oversized(max_bytes).await?;
        let excludes: Vec<String> = oversized
            .iter()
            .map(|path| format!(":(exclude,literal){path}"))
            .collect();
        let mut args = vec!["add", "-A", "--", "."];
        args.extend(excludes.iter().map(String::as_str));
        self.git(&args).await.map(drop)
    }

    async fn oversized(&self, max_bytes: Option<u64>) -> Result<Vec<String>> {
        let Some(max) = max_bytes else {
            return Ok(Vec::new());
        };
        let listed = self
            .git(&["ls-files", "-z", "-m", "-o", "--exclude-standard"])
            .await?;
        Ok(listed
            .split('\0')
            .filter(|path| !path.is_empty())
            .filter(|path| {
                std::fs::metadata(self.work_tree.join(path))
                    .is_ok_and(|meta| meta.is_file() && meta.len() > max)
            })
            .map(String::from)
            .collect())
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
    fn rejects_option_like_refs() {
        assert!(valid_ref("abc123"));
        assert!(!valid_ref("--all"));
        assert!(!valid_ref("a\nb"));
        assert!(!valid_ref(" "));
    }
}
