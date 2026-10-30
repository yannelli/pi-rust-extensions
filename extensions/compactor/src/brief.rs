//! Brief transcript: truncated, collapsed rendering of blocks.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::bash::compress_bash;
use crate::brief_collapse::{cap_tool_calls, collapse_errors, collapse_repeated_tools};
use crate::text::{Block, Kind, extract_path};

const TRUNCATE_USER: usize = 256;
const CLOSING_HEAD_WORDS: usize = 120;
const CLOSING_TAIL_WORDS: usize = 120;
const HEAD_WORDS: usize = 80;
const TAIL_WORDS: usize = 120;

static WORD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\p{L}\p{N}_]+(?:['’][\p{L}\p{N}_]+)*").unwrap());
static SELF_TALK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^\s*(?:hmm|wait|actually|oh|okay|ok|well|so)[,.!\s-]+").unwrap()
});
static SKILL_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<skill\s+name="([^"]+)"[^>]*>[\s\S]*?(?:</skill>|$)"#).unwrap());
static CR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\r\n?").unwrap());
static HSPACE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[^\S\n]+").unwrap());
static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());

pub static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    "a an the is are was were be been being have has had do does did will would could should may might shall can \
     need must to of in for on with at by from as into through during before after above below between under over \
     and but or nor not so yet both either neither each every all any few more most other some such no that this \
     these those it its i me my we our you your he him his she her they them their who which what if then than when \
     where how just also"
        .split_whitespace()
        .collect()
});

fn normalize_for_budget(text: &str) -> String {
    let text = CR.replace_all(text, "\n");
    let text = HSPACE.replace_all(&text, " ");
    BLANKS.replace_all(&text, "\n\n").trim().to_owned()
}

fn significant_words(flat: &str) -> Vec<(usize, usize)> {
    WORD.find_iter(flat)
        .filter(|m| !STOP_WORDS.contains(m.as_str().to_lowercase().as_str()))
        .map(|m| (m.start(), m.end()))
        .collect()
}

fn truncate_words(text: &str, limit: usize) -> String {
    let flat = normalize_for_budget(text);
    match significant_words(&flat).get(limit) {
        Some((start, _)) => format!("{}...(truncated)", flat[..*start].trim_end()),
        None => flat,
    }
}

fn truncate_head_tail(text: &str, head: usize, tail: usize) -> String {
    let flat = normalize_for_budget(text);
    let words = significant_words(&flat);
    if words.len() <= head + tail {
        return flat;
    }
    let head_text = flat[..words[head - 1].1].trim_end();
    let tail_text = flat[words[words.len() - tail].0..].trim_start();
    format!("{head_text}\n...(middle truncated)...\n{tail_text}")
}

fn collapse_skill_text(text: &str) -> String {
    SKILL_BLOCK.replace_all(text, "[skill: $1]").into_owned()
}

fn is_segment_closing(blocks: &[Block], index: usize) -> bool {
    blocks[index].kind == Kind::Assistant
        && blocks[index + 1..]
            .iter()
            .find(|b| b.kind != Kind::ToolResult)
            .is_none_or(|b| b.kind == Kind::User)
}

fn clip_dots(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let head: String = text.chars().take(max - 3).collect();
    format!("{head}...")
}

fn tool_one_liner(block: &Block) -> String {
    let name = block.name.as_str();
    let field = match name {
        "Read" | "Edit" | "Write" | "read" | "edit" | "write" => Some("file_path"),
        "Glob" | "Grep" => Some("pattern"),
        _ => None,
    };
    if let Some(value) = field.and_then(|f| block.arg_str(f)) {
        return format!("* {name} \"{value}\"");
    }
    if let Some(path) = extract_path(&block.args) {
        return format!("* {name} \"{path}\"");
    }
    if name == "bash" || name == "Bash" {
        let raw = block
            .arg_str("command")
            .or_else(|| block.arg_str("description"))
            .unwrap_or_default();
        return format!("* {name} \"{}\"", compress_bash(raw));
    }
    if let Some(query) = block.arg_str("query") {
        return format!("* {name} \"{}\"", clip_dots(query, 60));
    }
    format!("* {name}")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub header: String,
    pub lines: Vec<String>,
}

struct Builder {
    sections: Vec<Section>,
    last_header: String,
}

impl Builder {
    fn push(&mut self, header: &str, line: String) {
        if header == self.last_header
            && let Some(last) = self.sections.last_mut()
        {
            last.lines.push(line);
            return;
        }
        self.sections.push(Section {
            header: header.to_owned(),
            lines: vec![line],
        });
        self.last_header = header.to_owned();
    }

    fn push_text(&mut self, header: &str, text: &str, reference: &str) {
        let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
        if let Some(last) = lines.last_mut() {
            last.push_str(reference);
        }
        for line in lines {
            self.push(header, line);
        }
    }
}

fn reference(block: &Block) -> String {
    block.source.map(|s| format!(" (#{s})")).unwrap_or_default()
}

pub fn build_sections(blocks: &[Block]) -> Vec<Section> {
    let mut b = Builder {
        sections: Vec::new(),
        last_header: String::new(),
    };
    for (index, block) in blocks.iter().enumerate() {
        match block.kind {
            Kind::User => {
                if block.text.trim().is_empty() {
                    continue;
                }
                let text = truncate_words(&collapse_skill_text(&block.text), TRUNCATE_USER);
                if !text.is_empty() {
                    b.push_text("[user]", &text, &reference(block));
                }
                b.last_header = "[user]".into();
            }
            Kind::Assistant => {
                let mut raw = block.text.clone();
                for _ in 0..2 {
                    let stripped = SELF_TALK.replace(&raw, "").into_owned();
                    if stripped == raw {
                        break;
                    }
                    raw = stripped;
                }
                let text = if is_segment_closing(blocks, index) {
                    truncate_head_tail(&raw, CLOSING_HEAD_WORDS, CLOSING_TAIL_WORDS)
                } else {
                    truncate_head_tail(&raw, HEAD_WORDS, TAIL_WORDS)
                };
                if !text.is_empty() {
                    b.push_text("[assistant]", &text, &reference(block));
                }
            }
            Kind::ToolCall if !block.name.trim().is_empty() => {
                b.push(
                    "[assistant]",
                    format!("{}{}", tool_one_liner(block), reference(block)),
                );
            }
            Kind::ToolResult if block.is_error => {
                let body = block
                    .text
                    .split('\n')
                    .map(str::trim)
                    .find(|l| !l.is_empty())
                    .unwrap_or_default();
                if body.is_empty() || body == "(no output)" {
                    continue;
                }
                let header = format!("[tool_error] {}{}", block.name, reference(block));
                b.push(&header, body.to_owned());
                b.last_header = header;
            }
            _ => {}
        }
    }
    let mut sections = b.sections;
    for section in sections.iter_mut().filter(|s| s.header == "[assistant]") {
        section.lines = collapse_repeated_tools(std::mem::take(&mut section.lines));
        section.lines = cap_tool_calls(std::mem::take(&mut section.lines));
    }
    collapse_errors(sections)
}

fn all_tools(section: &Section) -> bool {
    section.header == "[assistant]" && section.lines.iter().all(|l| l.starts_with("* "))
}

pub fn stringify(sections: &[Section]) -> String {
    let mut out: Vec<&str> = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        if i > 0 && !(all_tools(&sections[i - 1]) && all_tools(section)) {
            out.push("");
        }
        out.push(&section.header);
        out.extend(section.lines.iter().map(String::as_str));
    }
    out.join("\n")
}

pub fn compile_brief(blocks: &[Block]) -> String {
    stringify(&build_sections(blocks))
}
