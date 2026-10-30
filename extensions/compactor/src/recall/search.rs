//! Hybrid search over blocks: a regex query first, then BM25 over terms.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::text::{Block, Kind, extract_path};

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub index: usize,
    pub score: f64,
    pub text: String,
    pub kind: &'static str,
    pub snippet: Option<String>,
    pub files: Vec<String>,
}

static METACHARS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[|*+?{}()\[\]\\^$.]").unwrap());

/// Case-insensitive regex for `pattern`, or its escaped literal when it does not compile.
pub fn safe_regex(pattern: &str) -> Regex {
    Regex::new(&format!("(?i){pattern}"))
        .or_else(|_| Regex::new(&format!("(?i){}", regex::escape(pattern))))
        .expect("escaped pattern compiles")
}

const STOPWORDS: &str = "the a an is are was were be been being have has had do does did will would could should may \
might can shall of in to for with on at from by as into through during before after above below between out off over \
under again further then once here there when where why how all both each few more most other some such no nor not only \
own same so than too very just about it its that this what which who whom these those";

fn filter_stopwords(terms: Vec<&str>) -> Vec<&str> {
    let stop: Vec<&str> = STOPWORDS.split_whitespace().collect();
    let meaningful: Vec<&str> = terms
        .iter()
        .copied()
        .filter(|t| !stop.contains(&t.to_lowercase().as_str()) && t.chars().count() > 1)
        .collect();
    if meaningful.is_empty() {
        terms
    } else {
        meaningful
    }
}

fn block_text(block: &Block) -> String {
    match block.kind {
        Kind::ToolCall => format!("{} {}", block.name, block.args),
        Kind::ToolResult => format!("{} {}", block.name, block.text),
        _ => block.text.clone(),
    }
}

/// `±2` lines around the first matching line, with counts of the lines left out.
fn line_snippet(text: &str, regex: &Regex) -> Option<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let at = lines.iter().position(|l| regex.is_match(l))?;
    let start = at.saturating_sub(2);
    let end = (at + 3).min(lines.len());
    let mut parts: Vec<String> = Vec::new();
    if start > 0 {
        parts.push(format!("...({start} lines above)"));
    }
    parts.extend(lines[start..end].iter().map(|l| (*l).to_owned()));
    if end < lines.len() {
        parts.push(format!("...({} lines below)", lines.len() - end));
    }
    Some(parts.join("\n"))
}

struct Doc {
    index: usize,
    text: String,
    kind: &'static str,
    files: Vec<String>,
}

impl Doc {
    fn hay(&self) -> String {
        format!("{} {} {}", self.kind, self.text, self.files.join(" "))
    }

    fn hit(&self, score: f64, snippet: Option<String>) -> Hit {
        Hit {
            index: self.index,
            score,
            text: self.text.clone(),
            kind: self.kind,
            snippet,
            files: self.files.clone(),
        }
    }
}

fn docs(blocks: &[Block]) -> Vec<Doc> {
    blocks
        .iter()
        .enumerate()
        .map(|(i, b)| Doc {
            index: b.source.unwrap_or(i),
            text: block_text(b),
            kind: b.kind.as_str(),
            files: if b.kind == Kind::ToolCall {
                extract_path(&b.args)
                    .into_iter()
                    .map(str::to_owned)
                    .collect()
            } else {
                Vec::new()
            },
        })
        .collect()
}

/// All blocks as hits when `query` is empty; else regex hits, else BM25-ranked term hits.
pub fn search(blocks: &[Block], query: Option<&str>) -> Vec<Hit> {
    let all = docs(blocks);
    let Some(query) = query.map(str::trim).filter(|q| !q.is_empty()) else {
        return all.iter().map(|d| d.hit(0.0, None)).collect();
    };
    if METACHARS.is_match(query) {
        let regex = safe_regex(query);
        let hits: Vec<Hit> = all
            .iter()
            .filter(|d| regex.is_match(&d.hay()))
            .map(|d| d.hit(1.0, line_snippet(&d.text, &regex)))
            .collect();
        if !hits.is_empty() {
            return hits;
        }
    }
    let terms = filter_stopwords(query.split_whitespace().collect());
    let regexes: Vec<Regex> = terms.iter().map(|t| safe_regex(t)).collect();
    let alternation = regexes
        .iter()
        .map(|r| r.as_str().trim_start_matches("(?i)").to_owned())
        .collect::<Vec<_>>()
        .join("|");
    let snippet_re = safe_regex(&alternation);
    let hays: Vec<String> = all.iter().map(Doc::hay).collect();
    let n = hays.len() as f64;
    let total_len: usize = hays
        .iter()
        .map(|h| h.split_whitespace().count().max(1))
        .sum();
    let avg_dl = total_len as f64 / n.max(1.0);
    let mut df: HashMap<usize, f64> = HashMap::new();
    for hay in &hays {
        for (t, re) in regexes.iter().enumerate() {
            if re.is_match(hay) {
                *df.entry(t).or_default() += 1.0;
            }
        }
    }
    let mut scored: Vec<Hit> = Vec::new();
    for (doc, hay) in all.iter().zip(&hays) {
        if !regexes.iter().any(|re| re.is_match(hay)) {
            continue;
        }
        let dl = hay.split_whitespace().count().max(1) as f64;
        let score: f64 = regexes
            .iter()
            .enumerate()
            .map(|(t, re)| {
                let tf = re.find_iter(hay).count() as f64;
                if tf == 0.0 {
                    return 0.0;
                }
                let doc_freq = df.get(&t).copied().unwrap_or(0.0);
                let idf = ((n - doc_freq + 0.5) / (doc_freq + 0.5) + 1.0).ln();
                idf * (tf * 2.2) / (tf + 1.2 * (0.25 + 0.75 * dl / avg_dl))
            })
            .sum();
        scored.push(doc.hit(score, line_snippet(&doc.text, &snippet_re)));
    }
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    scored
}
