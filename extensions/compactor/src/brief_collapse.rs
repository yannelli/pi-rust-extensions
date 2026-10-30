//! Brief transcript clean-up: repeated tool lines, per-turn tool caps, repeated errors.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::brief::Section;

const TOOL_CALLS_PER_TURN: usize = 8;

static TRAILING_REF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\(#(\d+)\)$").unwrap());
static REPEATED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.*) \((#[\d, #]+)\) x(\d+)$").unwrap());

pub(crate) fn collapse_repeated_tools(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in lines {
        if !line.starts_with("* ") {
            out.push(line);
            continue;
        }
        let r = TRAILING_REF
            .captures(&line)
            .map(|c| c[1].to_owned())
            .unwrap_or_default();
        let base = if r.is_empty() {
            line.clone()
        } else {
            line[..line.len() - (r.len() + 3)].trim_end().to_owned()
        };
        let last = out.last().cloned().unwrap_or_default();
        if let Some(m) = REPEATED.captures(&last).filter(|m| m[1] == base) {
            let count: usize = m[3].parse().unwrap_or(1);
            *out.last_mut().expect("last") = format!("{base} ({}, #{r}) x{}", &m[2], count + 1);
        } else if let Some(prev) = TRAILING_REF.captures(&last)
            && TRAILING_REF.replace(&last, "").trim_end() == base
        {
            *out.last_mut().expect("last") = format!("{base} (#{}, #{r}) x2", &prev[1]);
        } else {
            out.push(line);
        }
    }
    out
}

pub(crate) fn cap_tool_calls(lines: Vec<String>) -> Vec<String> {
    let tools: Vec<usize> = (0..lines.len())
        .filter(|i| lines[*i].starts_with("* "))
        .collect();
    if tools.len() <= TOOL_CALLS_PER_TURN {
        return lines;
    }
    let drop_count = tools.len() - TOOL_CALLS_PER_TURN;
    let dropped: HashSet<usize> = tools[..drop_count].iter().copied().collect();
    let first_kept = tools[drop_count];
    let mut out = Vec::new();
    for (i, line) in lines.into_iter().enumerate() {
        if dropped.contains(&i) {
            continue;
        }
        if i == first_kept {
            out.push(format!(
                "* ({drop_count} earlier tool-call entries omitted)"
            ));
        }
        out.push(line);
    }
    out
}

static ERROR_HEADER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[tool_error\]\s+(\S+?)(?:\s*\(#(\d+)\))?$").unwrap());
static ERROR_GROUP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[tool_error\]\s+(\S+?)\s*\(((?:#\d+(?:,\s*)?)+)\)(?:\s*x(\d+))?$").unwrap()
});

pub(crate) fn collapse_errors(sections: Vec<Section>) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for section in sections {
        let Some(m) = ERROR_HEADER
            .captures(&section.header)
            .filter(|_| section.lines.len() == 1)
        else {
            out.push(section);
            continue;
        };
        let (tool, r) = (m[1].to_owned(), m.get(2).map(|x| x.as_str().to_owned()));
        if let Some(prev) = out.last_mut()
            && let Some(pm) = ERROR_GROUP.captures(&prev.header.clone())
            && pm[1] == tool
            && prev.lines.len() == 1
            && prev.lines[0] == section.lines[0]
        {
            let refs = format!(
                "{}{}",
                &pm[2],
                r.map(|r| format!(", #{r}")).unwrap_or_default()
            );
            let count = pm
                .get(3)
                .and_then(|c| c.as_str().parse::<usize>().ok())
                .map_or(2, |c| c + 1);
            prev.header = format!("[tool_error] {tool} ({refs}) x{count}");
            continue;
        }
        out.push(section);
    }
    out
}
