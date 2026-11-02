//! Compact rows for the built-in tools: a status mark and a title, then the output behind a
//! `⎿` gutter.

use std::time::Duration;

use pi_ext_sdk::render::ToolRenderContext;
use pi_ext_sdk::style::{truncate, visible_width};
use pi_ext_sdk::{Rendered, SPINNER, ToolCallRender, ToolResultRender};
use serde_json::Value;

use crate::paint::Paint;
use crate::run::{elapsed, short};

/// The built-in tools this extension draws while no other renderer draws them.
pub const TOOLS: [&str; 7] = ["bash", "read", "edit", "write", "grep", "find", "ls"];

const GUTTER: &str = "  ⎿ ";
const INDENT: &str = "    ";

/// What a row needs besides its request.
pub struct Look<'a> {
    pub paint: &'a Paint,
    pub home: Option<&'a str>,
    /// pi draws [`SPINNER`] (feature `render.spinner`) and motion is on.
    pub spinner: bool,
    pub expand_key: &'a str,
    /// How long the call ran, or runs so far.
    pub timing: Option<Timing>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Timing {
    Running(Duration),
    Took(Duration),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Streaming,
    Waiting,
    Running,
    Done,
    Failed,
}

fn state(context: &ToolRenderContext) -> State {
    match context {
        ToolRenderContext {
            is_partial: false,
            is_error: true,
            ..
        } => State::Failed,
        ToolRenderContext {
            is_partial: false, ..
        } => State::Done,
        ToolRenderContext {
            args_complete: false,
            ..
        } => State::Streaming,
        ToolRenderContext {
            execution_started: false,
            ..
        } => State::Waiting,
        _ => State::Running,
    }
}

fn glyph(tool: &str) -> &'static str {
    match tool {
        "bash" => "$",
        "read" => "→",
        "edit" => "✎",
        "write" => "✚",
        "grep" | "find" => "⌕",
        "ls" => "▤",
        _ => "◆",
    }
}

/// `path` relative to `cwd`, or with `~` for the home directory.
fn display_path(path: &str, cwd: Option<&str>, home: Option<&str>) -> String {
    let inside = |base: &str| {
        path.strip_prefix(base)
            .and_then(|rest| rest.strip_prefix('/'))
    };
    if let Some(rest) = cwd.and_then(inside).filter(|rest| !rest.is_empty()) {
        return rest.to_owned();
    }
    match home.and_then(|home| inside(home).map(|rest| format!("~/{rest}"))) {
        Some(short) => short,
        None => path.to_owned(),
    }
}

fn text_arg<'a>(args: &'a Value, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The verb, the target, and muted details of a call.
fn title(
    tool: &str,
    args: &Value,
    cwd: Option<&str>,
    home: Option<&str>,
) -> (String, String, String) {
    let path = |key: &str| display_path(text_arg(args, key), cwd, home);
    let place = |key: &str| match text_arg(args, key) {
        "" | "." => String::new(),
        _ => format!(" in {}", path(key)),
    };
    match tool {
        "bash" => {
            let command = text_arg(args, "command").trim();
            let more = if command.contains('\n') { " …" } else { "" };
            (
                short(command.lines().next().unwrap_or(""), 400),
                String::new(),
                more.into(),
            )
        }
        "read" => {
            let offset = args.get("offset").and_then(Value::as_u64);
            let limit = args.get("limit").and_then(Value::as_u64);
            let range = match (offset, limit) {
                (Some(start), Some(count)) => {
                    format!(":{start}-{}", start + count.saturating_sub(1))
                }
                (Some(start), None) => format!(":{start}"),
                (None, Some(count)) => format!(":1-{count}"),
                (None, None) => String::new(),
            };
            ("Read".into(), format!(" {}", path("path")), range)
        }
        "edit" => ("Edit".into(), format!(" {}", path("path")), String::new()),
        "write" => ("Write".into(), format!(" {}", path("path")), String::new()),
        "grep" => {
            let glob = match text_arg(args, "glob") {
                "" => String::new(),
                glob => format!(" ({glob})"),
            };
            let target = format!(" \"{}\"", text_arg(args, "pattern"));
            ("Search".into(), target, format!("{}{glob}", place("path")))
        }
        "find" => (
            "Find".into(),
            format!(" {}", text_arg(args, "pattern")),
            place("path"),
        ),
        "ls" => {
            let target = match text_arg(args, "path") {
                "" | "." => " .".to_owned(),
                _ => format!(" {}", path("path")),
            };
            ("List".into(), target, String::new())
        }
        other => (other.into(), String::new(), String::new()),
    }
}

/// The call row: one line with the mark, the title, and the time of a running call.
pub fn call_row(req: &ToolCallRender, look: &Look) -> Rendered {
    let paint = look.paint;
    let state = state(&req.context);
    let mark = match state {
        State::Running if look.spinner => paint.fg("accent", SPINNER),
        State::Streaming if look.spinner => paint.fg("muted", SPINNER),
        State::Running | State::Streaming => paint.fg("accent", "●"),
        State::Waiting => paint.fg("warning", "◇"),
        State::Done => paint.fg("success", glyph(&req.tool_name)),
        State::Failed => paint.fg("error", "✗"),
    };
    let cwd = req.context.cwd.as_deref();
    let (verb, target, details) = title(&req.tool_name, &req.args, cwd, look.home);
    let suffix = match (state, look.timing) {
        (State::Running, Some(Timing::Running(time))) => format!(" · {}", elapsed(time)),
        (State::Done | State::Failed, Some(Timing::Took(time)))
            if time >= Duration::from_secs(1) =>
        {
            format!(" · {}", elapsed(time))
        }
        (State::Waiting, _) => " · waiting".into(),
        _ => String::new(),
    };
    let budget = req.width.saturating_sub(2 + visible_width(&suffix)).max(1);
    let verb = paint.fg("toolTitle", &Paint::bold(&verb));
    let body = truncate(
        &format!("{verb}{target}{}", paint.fg("muted", &details)),
        budget,
    );
    let line = format!("{mark} {body}{}", paint.fg("muted", &suffix));
    let rendered = Rendered::new(vec![line]);
    match state {
        State::Running => rendered.refresh_after(1000),
        _ => rendered,
    }
}

/// The result row: the output behind the gutter, cut to a few lines unless expanded.
pub fn result_row(req: &ToolResultRender, look: &Look) -> Option<Vec<String>> {
    let paint = look.paint;
    let expanded = req.options.expanded || req.context.expanded;
    let failed =
        req.context.is_error || req.result.get("isError").and_then(Value::as_bool) == Some(true);
    let text = req.text();
    let lines: Vec<&str> = text.trim_end().split('\n').collect();
    let width = req.width.saturating_sub(visible_width(GUTTER)).max(4);
    let more = |count: usize| {
        let key = look.expand_key;
        paint.fg(
            "muted",
            &format!(
                "… +{count} line{} ({key} to expand)",
                if count == 1 { "" } else { "s" }
            ),
        )
    };
    let body: Vec<String> = if failed {
        head(
            &lines,
            if expanded { 200 } else { 8 },
            |line| paint.fg("error", line),
            more,
        )
    } else {
        match req.tool_name.as_str() {
            "bash" if text.trim().is_empty() => match req.options.is_partial {
                true => return None,
                false => vec![paint.fg("muted", "(no output)")],
            },
            "bash" => tail(
                &lines,
                if expanded { 200 } else { 5 },
                |line| paint.fg("toolOutput", line),
                more,
            ),
            "read" if expanded => head(&lines, 200, |line| paint.fg("toolOutput", line), more),
            "read" => {
                let cut =
                    req.result.pointer("/details/truncation/truncated") == Some(&Value::Bool(true));
                let note = if cut { " (truncated)" } else { "" };
                vec![paint.fg("muted", &format!("{}{note}", count(lines.len(), "line")))]
            }
            "edit" => match req.result.pointer("/details/diff").and_then(Value::as_str) {
                Some(diff) => {
                    crate::diff::render(paint, diff, &req.context.args, width, expanded, more)
                }
                None => vec![paint.fg("muted", lines[0])],
            },
            "write" => {
                let content = text_arg(&req.context.args, "content");
                let written: Vec<&str> = content.trim_end().split('\n').collect();
                let mut out =
                    vec![paint.fg("muted", &format!("Wrote {}", count(written.len(), "line")))];
                if expanded {
                    out.extend(head(
                        &written,
                        200,
                        |line| paint.fg("toolOutput", line),
                        more,
                    ));
                }
                out
            }
            "grep" | "find" | "ls" => {
                let found = lines.iter().filter(|line| !line.trim().is_empty()).count();
                let unit = match req.tool_name.as_str() {
                    "grep" => "match",
                    "find" => "file",
                    _ => "entry",
                };
                let mut out = vec![paint.fg("muted", &count(found, unit))];
                if found > 0 {
                    out.extend(head(
                        &lines,
                        if expanded { 200 } else { 5 },
                        |line| paint.fg("toolOutput", line),
                        more,
                    ));
                }
                out
            }
            _ => return None,
        }
    };
    Some(gutter(paint, body, width))
}

fn count(n: usize, unit: &str) -> String {
    let plural = match unit {
        "match" => "matches",
        "entry" => "entries",
        "line" => "lines",
        _ => "files",
    };
    format!("{n} {}", if n == 1 { unit } else { plural })
}

/// The first `limit` lines and a hint for the rest.
fn head(
    lines: &[&str],
    limit: usize,
    style: impl Fn(&str) -> String,
    more: impl Fn(usize) -> String,
) -> Vec<String> {
    // A hint would take the row of the one line it hides.
    let limit = if lines.len() == limit + 1 {
        lines.len()
    } else {
        limit
    };
    let mut out: Vec<String> = lines.iter().take(limit).map(|line| style(line)).collect();
    if lines.len() > limit {
        out.push(more(lines.len() - limit));
    }
    out
}

/// The last `limit` lines after a hint for the lines above them.
fn tail(
    lines: &[&str],
    limit: usize,
    style: impl Fn(&str) -> String,
    more: impl Fn(usize) -> String,
) -> Vec<String> {
    let skip = match lines.len().saturating_sub(limit) {
        1 => 0,
        skip => skip,
    };
    let mut out = Vec::new();
    if skip > 0 {
        out.push(more(skip));
    }
    out.extend(lines[skip..].iter().map(|line| style(line)));
    out
}

/// `⎿` before the first line, an indent before the others; each line cut to `width`.
fn gutter(paint: &Paint, lines: Vec<String>, width: usize) -> Vec<String> {
    lines
        .into_iter()
        .enumerate()
        .map(|(at, line)| {
            let lead = if at == 0 {
                paint.fg("muted", GUTTER)
            } else {
                INDENT.to_owned()
            };
            format!("{lead}{}", truncate(&line.replace('\t', "  "), width))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_to_the_cwd_or_home() {
        let (cwd, home) = (Some("/work/app"), Some("/home/me"));
        assert_eq!(display_path("/work/app/src/a.rs", cwd, home), "src/a.rs");
        assert_eq!(display_path("/home/me/notes.md", cwd, home), "~/notes.md");
        assert_eq!(display_path("/etc/hosts", cwd, home), "/etc/hosts");
        assert_eq!(display_path("src/b.rs", cwd, home), "src/b.rs");
    }

    #[test]
    fn counts_and_cuts() {
        assert_eq!(count(1, "match"), "1 match");
        assert_eq!(count(3, "entry"), "3 entries");
        let lines = ["a", "b", "c"];
        let hint = |n: usize| format!("+{n}");
        assert_eq!(tail(&lines, 2, str::to_owned, hint), ["a", "b", "c"]);
        assert_eq!(tail(&lines, 1, str::to_owned, hint), ["+2", "c"]);
        assert_eq!(head(&lines, 1, str::to_owned, hint), ["a", "+2"]);
        assert_eq!(head(&lines, 2, str::to_owned, hint), ["a", "b", "c"]);
    }
}
