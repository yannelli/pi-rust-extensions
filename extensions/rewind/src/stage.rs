//! Staging in batches: each batch updates the shadow index, so a first snapshot of a large
//! work tree keeps its progress when the caller's deadline passes.

use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;

use pi_ext_sdk::{Result, bail, info, warn};

use crate::git::{ShadowRepo, unreadable_files};
use crate::gitcmd::{failure, git_output_within};

const BATCH_FILES: usize = 500;
const BATCH_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Staged {
    Complete,
    /// The deadline passed; `remaining` paths of this pass are not staged yet.
    Partial {
        staged: usize,
        remaining: usize,
    },
}

enum Added {
    Done,
    Failed(String),
    TimedOut,
}

struct Candidate<'a> {
    path: &'a str,
    tracked: bool,
}

impl ShadowRepo {
    /// Stages the work tree, skipping files over `max_bytes`. With a `deadline`, returns
    /// [`Staged::Partial`] after the first batch that ends past it.
    pub async fn stage(
        &self,
        patterns: &[String],
        max_bytes: Option<u64>,
        deadline: Option<Instant>,
    ) -> Result<Staged> {
        self.write_exclude(patterns).await?;
        self.unstage_ignored().await?;
        let untracked = self
            .git(&["ls-files", "-z", "-o", "--exclude-standard"])
            .await?;
        let changed = self.git(&["ls-files", "-z", "-m", "-d"]).await?;
        let work_tree = self.work_tree().to_path_buf();
        let pending = candidates(&work_tree, &untracked, &changed, max_bytes);
        let total = pending.len();
        let mut staged = 0;
        for batch in batches(&work_tree, &pending) {
            self.add_batch(batch).await?;
            staged += batch.len();
            if staged < total && deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                let remaining = total - staged;
                return Ok(Staged::Partial { staged, remaining });
            }
        }
        Ok(Staged::Complete)
    }

    async fn add_batch(&self, batch: &[Candidate<'_>]) -> Result<()> {
        match self.add_paths(batch.iter().map(|c| c.path)).await? {
            Added::Done => return Ok(()),
            Added::TimedOut => return self.timed_out(batch),
            Added::Failed(error) if !error.contains("did not match any files") => bail!("{error}"),
            Added::Failed(_) => {}
        }
        let work_tree = self.work_tree();
        let present = batch
            .iter()
            .filter(|c| c.tracked || std::fs::symlink_metadata(work_tree.join(c.path)).is_ok());
        match self.add_paths(present.map(|c| c.path)).await? {
            Added::Done => Ok(()),
            Added::TimedOut => self.timed_out(batch),
            Added::Failed(error) => bail!("{error}"),
        }
    }

    /// A lone file that git cannot store within the add timeout stays out of later snapshots.
    fn timed_out(&self, batch: &[Candidate<'_>]) -> Result<()> {
        self.clear_stale_locks();
        let limit = self.add_timeout().as_secs_f64();
        let [file] = batch else {
            bail!("git add of {} files took longer than {limit}s", batch.len());
        };
        self.skip_path(file.path)?;
        warn!(
            "checkpoints skip {}: git could not store it within {limit}s \
             (checkpoint.gitTimeoutSeconds, checkpoint.maxFileMB)",
            file.path
        );
        Ok(())
    }

    /// `git add -A` of `paths`.
    async fn add_paths<'a>(&self, paths: impl Iterator<Item = &'a str>) -> Result<Added> {
        let mut input = Vec::new();
        for path in paths {
            input.extend_from_slice(path.as_bytes());
            input.push(0);
        }
        if input.is_empty() {
            return Ok(Added::Done);
        }
        let add = [
            "add",
            "-A",
            "--ignore-errors",
            "--pathspec-from-file=-",
            "--pathspec-file-nul",
        ];
        let mut args = vec!["--literal-pathspecs"];
        args.extend(self.full_args(&add)?);
        let cwd = Some(self.work_tree());
        let output = git_output_within(cwd, &args, Some(&input), self.add_timeout()).await?;
        let Some(output) = output else {
            return Ok(Added::TimedOut);
        };
        if output.status.success() {
            return Ok(Added::Done);
        }
        match unreadable_files(&String::from_utf8_lossy(&output.stderr)) {
            Some(paths) => {
                info!("checkpoint skipped unreadable files: {}", paths.join(", "));
                Ok(Added::Done)
            }
            None => Ok(Added::Failed(failure(&args, &output))),
        }
    }
}

fn candidates<'a>(
    work_tree: &Path,
    untracked: &'a str,
    changed: &'a str,
    max_bytes: Option<u64>,
) -> Vec<Candidate<'a>> {
    let oversized = |path: &str| {
        max_bytes.is_some_and(|max| {
            std::fs::metadata(work_tree.join(path))
                .is_ok_and(|meta| meta.is_file() && meta.len() > max)
        })
    };
    let mut seen = HashSet::new();
    let untracked = untracked.split('\0').map(|path| (path, false));
    let changed = changed.split('\0').map(|path| (path, true));
    untracked
        .chain(changed)
        .filter(|(path, _)| !path.is_empty() && seen.insert(*path))
        .filter(|(path, _)| !oversized(path))
        .map(|(path, tracked)| Candidate { path, tracked })
        .collect()
}

/// Splits `paths` into runs of at most [`BATCH_FILES`] paths or about [`BATCH_BYTES`] bytes.
/// A file of [`BATCH_BYTES`] or more gets a batch of its own.
fn batches<'c, 'a>(work_tree: &Path, paths: &'c [Candidate<'a>]) -> Vec<&'c [Candidate<'a>]> {
    let mut out = Vec::new();
    let (mut start, mut bytes) = (0, 0u64);
    for (index, candidate) in paths.iter().enumerate() {
        let size = std::fs::symlink_metadata(work_tree.join(candidate.path)).map_or(0, |m| m.len());
        if size >= BATCH_BYTES && index > start {
            out.push(&paths[start..index]);
            (start, bytes) = (index, 0);
        }
        bytes = bytes.saturating_add(size);
        if index + 1 - start >= BATCH_FILES || bytes >= BATCH_BYTES {
            out.push(&paths[start..=index]);
            (start, bytes) = (index + 1, 0);
        }
    }
    if start < paths.len() {
        out.push(&paths[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn candidates_skip_duplicates_empty_and_oversized_paths() {
        let dir = TestDir::new("candidates");
        std::fs::write(dir.path().join("big.bin"), vec![0u8; 2048]).unwrap();
        std::fs::write(dir.path().join("small.txt"), "x").unwrap();
        let picked = candidates(
            dir.path(),
            "big.bin\0small.txt\0",
            "small.txt\0gone.txt\0",
            Some(1024),
        );
        let paths: Vec<(&str, bool)> = picked.iter().map(|c| (c.path, c.tracked)).collect();
        assert_eq!(paths, [("small.txt", false), ("gone.txt", true)]);
    }

    #[test]
    fn batches_split_by_count_and_bytes() {
        let dir = TestDir::new("batches");
        let names: Vec<String> = (0..BATCH_FILES + 3).map(|i| format!("f{i}")).collect();
        for name in &names {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        std::fs::write(dir.path().join("huge"), vec![0u8; BATCH_BYTES as usize]).unwrap();
        let mut paths: Vec<Candidate> = names
            .iter()
            .map(|path| Candidate {
                path,
                tracked: false,
            })
            .collect();
        paths.insert(
            1,
            Candidate {
                path: "huge",
                tracked: false,
            },
        );
        let sizes: Vec<usize> = batches(dir.path(), &paths)
            .iter()
            .map(|b| b.len())
            .collect();
        assert_eq!(sizes, [1, 1, BATCH_FILES, 2]);
    }

    #[tokio::test]
    async fn a_batch_with_a_vanished_file_stages_the_rest() {
        let work = TestDir::new("vanish");
        let state = TestDir::new("vanish-state");
        std::fs::write(work.path().join("a.txt"), "a").unwrap();
        let repo = ShadowRepo::new(state.path(), work.path());
        repo.ensure().await.unwrap();
        let batch = [
            Candidate {
                path: "gone.txt",
                tracked: false,
            },
            Candidate {
                path: "a.txt",
                tracked: false,
            },
        ];
        repo.add_batch(&batch).await.unwrap();
        assert_eq!(repo.git(&["ls-files"]).await.unwrap().trim(), "a.txt");
    }
}
