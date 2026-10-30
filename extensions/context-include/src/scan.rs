use std::collections::HashSet;

use serde::Deserialize;

use crate::config::Config;
use crate::fmt::js_number;
use crate::pathx::{self, canonicalize, dirname, expand_tilde, is_absolute, resolve};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct ContextFile {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContextFileDiag {
    pub path: String,
    pub exists: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub reference: String,
    pub resolved_path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diagnostic {
    pub context_files: Vec<ContextFileDiag>,
    pub included: Vec<(String, u64)>,
    pub skipped: Vec<Skipped>,
    pub total_bytes: u64,
    pub max_bytes: f64,
    pub max_depth: f64,
    pub allowed_roots: Vec<String>,
    pub denied_roots: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Included {
    pub path: String,
    pub content: String,
}

pub struct ScanOutput {
    pub section: Option<String>,
    pub diag: Diagnostic,
}

/// Paths and limits for one scan; the filesystem is read directly.
pub struct Scanner {
    pub home: String,
    pub agent_dir: String,
    pub cwd: String,
    pub config: Config,
}

struct State {
    visited: HashSet<String>,
    results: Vec<Included>,
    diag: Diagnostic,
    bytes: u64,
}

/// `@path.md` lines outside code fences, after an optional list marker.
pub fn extract_references(content: &str) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in content.split('\n') {
        let trimmed = line.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some(r) = match_reference(strip_list_marker(trimmed))
            && !refs.iter().any(|seen| seen == r)
        {
            refs.push(r.to_owned());
        }
    }
    refs
}

fn strip_list_marker(line: &str) -> &str {
    let rest = if let Some(rest) = line.strip_prefix(['-', '*', '+']) {
        rest
    } else {
        let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        match line[digits..].strip_prefix('.') {
            Some(rest) if digits > 0 => rest,
            _ => return line,
        }
    };
    let after = rest.trim_start_matches(char::is_whitespace);
    if after.len() < rest.len() {
        after
    } else {
        line
    }
}

/// `^@([~.]?[\w./-]+\.md)$`
fn match_reference(line: &str) -> Option<&str> {
    let reference = line.strip_prefix('@')?;
    let body = reference.strip_prefix('~').unwrap_or(reference);
    let ok = body.len() > 3
        && body.ends_with(".md")
        && body
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'));
    ok.then_some(reference)
}

impl Scanner {
    pub fn effective_roots(&self, files: &[ContextFile]) -> (Vec<String>, Vec<String>) {
        let denied = self.resolve_roots(&self.config.denied_roots);
        let allowed = match &self.config.allowed_roots {
            Some(roots) => self.resolve_roots(roots),
            None => {
                let mut roots = vec![canonicalize(
                    &format!("{}/includes", self.agent_dir),
                    &self.cwd,
                )];
                for path in files
                    .iter()
                    .filter_map(|f| f.path.as_deref())
                    .filter(|p| !p.is_empty())
                {
                    let dir = canonicalize(&dirname(&resolve(&self.cwd, path)), &self.cwd);
                    if !roots.contains(&dir) {
                        roots.push(dir);
                    }
                }
                roots
            }
        };
        (allowed, denied)
    }

    fn resolve_roots(&self, roots: &[String]) -> Vec<String> {
        roots
            .iter()
            .map(|root| canonicalize(&expand_tilde(root, &self.home), &self.cwd))
            .collect()
    }

    pub fn scan(&self, files: &[ContextFile]) -> ScanOutput {
        let (allowed_roots, denied_roots) = self.effective_roots(files);
        let mut state = State {
            visited: HashSet::new(),
            results: Vec::new(),
            bytes: 0,
            diag: Diagnostic {
                max_bytes: self.config.max_bytes,
                max_depth: self.config.max_depth,
                allowed_roots,
                denied_roots,
                ..Diagnostic::default()
            },
        };
        for file in files {
            let Some(path) = file.path.as_deref().filter(|p| !p.is_empty()) else {
                state.diag.context_files.push(ContextFileDiag {
                    path: "<empty>".into(),
                    exists: false,
                    error: Some("empty path".into()),
                });
                continue;
            };
            let path = resolve(&self.cwd, path);
            let exists = std::fs::canonicalize(&path).is_ok();
            state.diag.context_files.push(ContextFileDiag {
                path: path.clone(),
                exists,
                error: None,
            });
            if exists {
                self.resolve_includes(&path, &file.content, &mut state, 0);
            }
        }
        if state.results.is_empty() {
            return ScanOutput {
                section: None,
                diag: state.diag,
            };
        }
        state.diag.total_bytes = state.bytes;
        let blocks: Vec<String> = state
            .results
            .iter()
            .map(|inc| {
                format!(
                    "<project_instructions path=\"{}\">\n{}\n</project_instructions>",
                    inc.path, inc.content
                )
            })
            .collect();
        let section = format!(
            "Additional files included via @-syntax references in the project instructions:\n\n{}",
            blocks.join("\n\n")
        );
        ScanOutput {
            section: Some(section),
            diag: state.diag,
        }
    }

    fn resolve_includes(&self, file: &str, content: &str, state: &mut State, depth: u32) {
        if f64::from(depth) > self.config.max_depth {
            let rel = pathx::relative(&self.cwd, file);
            state.diag.skipped.push(Skipped {
                reference: if rel.is_empty() { file.to_owned() } else { rel },
                resolved_path: file.to_owned(),
                reason: format!("max depth ({}) exceeded", js_number(self.config.max_depth)),
            });
            return;
        }
        let canonical = canonicalize(file, &self.cwd);
        if !state.visited.insert(canonical.clone()) {
            return;
        }
        let base = dirname(&canonical);
        for reference in extract_references(content) {
            let expanded = expand_tilde(&reference, &self.home);
            let resolved = if is_absolute(&expanded) {
                expanded
            } else {
                resolve(&base, &expanded)
            };
            let real = canonicalize(&resolved, &self.cwd);
            let skip = |state: &mut State, reason: String| {
                state.diag.skipped.push(Skipped {
                    reference: reference.clone(),
                    resolved_path: resolved.clone(),
                    reason,
                });
            };
            if state.visited.contains(&real) {
                skip(state, "already included".into());
                continue;
            }
            if pathx::under_any_root(&real, &state.diag.denied_roots) {
                skip(state, "denied by deniedRoots".into());
                continue;
            }
            if !pathx::under_any_root(&real, &state.diag.allowed_roots) {
                skip(state, "outside allowed roots".into());
                continue;
            }
            let text = match std::fs::read(&real) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    skip(state, "file not found".into());
                    continue;
                }
                Err(err) => {
                    skip(state, format!("read error: {err}"));
                    continue;
                }
            };
            if text.is_empty() {
                skip(state, "file is empty".into());
                continue;
            }
            let size = text.len() as u64;
            if (state.bytes + size) as f64 > self.config.max_bytes {
                skip(
                    state,
                    format!(
                        "size limit exceeded (would reach {} bytes)",
                        state.bytes + size
                    ),
                );
                continue;
            }
            state.bytes += size;
            state.diag.included.push((resolved.clone(), size));
            state.results.push(Included {
                path: resolved.clone(),
                content: text.clone(),
            });
            self.resolve_includes(&real, &text, state, depth + 1);
        }
    }
}
