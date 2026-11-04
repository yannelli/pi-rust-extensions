//! Characters saved per tool, for this session and in total (`ext/tool-results/stats.json`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::NAME;
use crate::pipeline::Outcome;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ToolStats {
    pub calls: u64,
    pub changed: u64,
    pub truncated: u64,
    pub chars_in: u64,
    pub chars_out: u64,
    /// Characters a dry run would have saved.
    pub dry_saved: u64,
}

impl ToolStats {
    pub fn saved(&self) -> u64 {
        self.chars_in.saturating_sub(self.chars_out)
    }

    fn add(&mut self, outcome: Option<&Outcome>, size: usize, dry: bool) {
        self.calls += 1;
        let Some(outcome) = outcome else {
            self.chars_in += size as u64;
            self.chars_out += size as u64;
            return;
        };
        if dry {
            self.dry_saved += outcome.before.saturating_sub(outcome.after) as u64;
            self.chars_in += size as u64;
            self.chars_out += size as u64;
            return;
        }
        self.changed += 1;
        self.truncated += u64::from(outcome.truncated);
        self.chars_in += outcome.before as u64;
        self.chars_out += outcome.after as u64;
    }
}

pub type Table = BTreeMap<String, ToolStats>;

/// The last processed result, for `/tool-results preview`.
#[derive(Clone, Debug, Default)]
pub struct Last {
    pub tool: String,
    pub before: usize,
    pub after: usize,
    pub applied: Vec<String>,
    pub dry: bool,
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Stats {
    pub session: Table,
    pub total: Table,
    pub last: Option<Last>,
}

pub fn path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join(NAME).join("stats.json")
}

impl Stats {
    pub fn load(file: &Path) -> Self {
        let total = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        Self {
            total,
            ..Self::default()
        }
    }

    pub fn save(&self, file: &Path) -> std::io::Result<()> {
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(
            file,
            serde_json::to_string_pretty(&self.total).unwrap_or_default(),
        )
    }

    /// Counts one result of `tool` with `size` text characters.
    pub fn record(&mut self, tool: &str, outcome: Option<&Outcome>, size: usize, dry: bool) {
        for table in [&mut self.session, &mut self.total] {
            table
                .entry(tool.to_owned())
                .or_default()
                .add(outcome, size, dry);
        }
        if let Some(outcome) = outcome {
            let text = outcome.content.iter().find(|b| b["type"] == "text");
            let text = text.and_then(|b| b["text"].as_str()).unwrap_or_default();
            self.last = Some(Last {
                tool: tool.to_owned(),
                before: outcome.before,
                after: outcome.after,
                applied: outcome.applied.clone(),
                dry,
                text: text.chars().take(4000).collect(),
            });
        }
    }

    pub fn reset(&mut self) {
        self.session.clear();
        self.total.clear();
        self.last = None;
    }
}

pub fn sum(table: &Table) -> ToolStats {
    table.values().fold(ToolStats::default(), |mut acc, s| {
        acc.calls += s.calls;
        acc.changed += s.changed;
        acc.truncated += s.truncated;
        acc.chars_in += s.chars_in;
        acc.chars_out += s.chars_out;
        acc.dry_saved += s.dry_saved;
        acc
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(before: usize, after: usize, truncated: bool) -> Outcome {
        Outcome {
            before,
            after,
            truncated,
            ..Outcome::default()
        }
    }

    #[test]
    fn records_session_and_total() {
        let dir = tempfile::tempdir().unwrap();
        let file = path(dir.path());
        let mut stats = Stats::load(&file);
        stats.record("bash", Some(&outcome(1000, 200, false)), 1000, false);
        stats.record("bash", None, 50, false);
        stats.record("bash", Some(&outcome(30_000, 5_000, true)), 30_000, false);
        stats.record("grep", Some(&outcome(100, 60, false)), 100, true);
        let bash = &stats.session["bash"];
        assert_eq!((bash.calls, bash.changed, bash.truncated), (3, 2, 1));
        assert_eq!(bash.saved(), 25_800);
        assert_eq!(stats.session["grep"].dry_saved, 40);
        assert_eq!(stats.session["grep"].saved(), 0);
        assert_eq!(sum(&stats.session).saved(), 25_800);
        stats.save(&file).unwrap();
        let again = Stats::load(&file);
        assert_eq!(again.total, stats.total);
        assert!(again.session.is_empty());
        stats.reset();
        assert!(stats.total.is_empty() && stats.last.is_none());
    }
}
