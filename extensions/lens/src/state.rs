use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::Config;
use crate::pipeline::Runner;

pub const MAX_GUARD_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Default)]
pub struct State {
    pub config: Config,
    pub findings: BTreeMap<PathBuf, Vec<Value>>,
    pub guard: Guard,
    pub warned_trust: bool,
    pub runner: Runner,
}

/// Text of a regular file of at most [`MAX_GUARD_BYTES`]. Lens does not open
/// FIFOs, devices or large files, because a read can block or use much memory.
pub fn small_text(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_GUARD_BYTES {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

#[derive(Clone, Default, Deserialize, Serialize)]
pub struct Guard {
    reads: HashMap<PathBuf, Read>,
    #[serde(skip)]
    pub allowed: HashSet<PathBuf>,
}

#[derive(Clone, Deserialize, Serialize)]
struct Read {
    text: String,
    ranges: Vec<(usize, usize)>,
}

impl Guard {
    /// `text` is the current file text from [`small_text`].
    pub fn record(&mut self, path: &Path, text: Option<String>, input: &Value, content: &[Value]) {
        let Some(text) = text else {
            return;
        };
        let start = input["offset"].as_u64().unwrap_or(1).max(1) as usize;
        let total = text.lines().count().max(1);
        let output = content
            .iter()
            .filter_map(|item| item["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n");
        if output.is_empty() {
            return;
        }
        let truncated = output.contains("truncated")
            || output.contains("Use offset=")
            || output.contains("Use offset ");
        let requested = input["limit"].as_u64().map(|limit| limit as usize);
        let count = if truncated {
            output.lines().count().saturating_sub(2)
        } else {
            requested.unwrap_or(total)
        };
        if count == 0 {
            return;
        }
        let range = (start, start.saturating_add(count - 1).min(total));
        let read = self
            .reads
            .entry(path.to_path_buf())
            .or_insert_with(|| Read {
                text: text.clone(),
                ranges: vec![],
            });
        if read.text != text {
            read.ranges.clear();
            read.text = text;
        }
        read.ranges.push(range);
    }

    /// `text` is the current file text from [`small_text`]. Missing, large,
    /// special and non-UTF-8 files have none and pass without a check.
    pub fn block_reason(
        &mut self,
        path: &Path,
        text: Option<String>,
        tool: &str,
        input: &Value,
    ) -> Option<String> {
        if self.allowed.remove(path) {
            return None;
        }
        let text = text?;
        let covered = self
            .reads
            .get(path)
            .filter(|read| read.text == text)
            .is_some_and(|read| {
                let mut ranges = read.ranges.clone();
                ranges.sort_unstable();
                let mut end = 0;
                for (start, stop) in ranges {
                    if start > end + 1 {
                        break;
                    }
                    end = end.max(stop);
                }
                if end >= read.text.lines().count().max(1) {
                    return true;
                }
                if tool != "edit" {
                    return false;
                }
                input["oldText"]
                    .as_str()
                    .filter(|old| !old.is_empty())
                    .is_some_and(|old| {
                        let matches: Vec<_> = read.text.match_indices(old).collect();
                        matches.len() == 1
                            && matches.iter().all(|(offset, old)| {
                                let first =
                                    read.text[..*offset].bytes().filter(|b| *b == b'\n').count()
                                        + 1;
                                let last = first + old.bytes().filter(|b| *b == b'\n').count();
                                read.ranges
                                    .iter()
                                    .any(|(start, end)| *start <= first && *end >= last)
                            })
                    })
            });
        (!covered).then(|| format!("Read {} before {tool}; Lens requires current file context. Use /lens-allow-edit <path> for one edit.", path.display()))
    }

    pub fn invalidate(&mut self, path: &Path) {
        self.reads.remove(path);
    }
}

impl State {
    pub fn all_findings(&self) -> Vec<Value> {
        self.findings.values().flatten().cloned().collect()
    }
}
