//! Codex execpolicy `.rules` files: the `prefix_rule` subset of Starlark.

use crate::rules::{Decision, NetworkRule, PrefixRule};
use crate::shell;

#[derive(Debug, Clone, PartialEq)]
pub struct CodexRule {
    pub rule: PrefixRule,
    pub decision: Decision,
    pub justification: Option<String>,
    pub text: String,
}

#[derive(Debug, Default, PartialEq)]
pub struct Parsed {
    pub rules: Vec<CodexRule>,
    /// Calls the extension reads past without applying.
    pub skipped: Vec<String>,
    /// `host_executable` names with their absolute paths.
    pub hosts: Vec<(String, Vec<String>)>,
    pub network: Vec<(NetworkRule, Decision, Option<String>, String)>,
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    Str(String),
    List(Vec<Val>),
    Other(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Punct(char),
}

pub fn parse_rules(text: &str) -> Result<Parsed, String> {
    let toks = tokenize(text)?;
    let mut parsed = Parsed::default();
    let mut i = 0;
    while i < toks.len() {
        let Tok::Ident(name) = &toks[i] else {
            return Err(format!("expected a function call, found {:?}", toks[i]));
        };
        if toks.get(i + 1) != Some(&Tok::Punct('(')) {
            return Err(format!("unsupported statement starting with `{name}`"));
        }
        i += 2;
        let mut args: Vec<(Option<String>, Val)> = Vec::new();
        while toks.get(i) != Some(&Tok::Punct(')')) {
            let key = match (toks.get(i), toks.get(i + 1)) {
                (Some(Tok::Ident(key)), Some(Tok::Punct('='))) => {
                    i += 2;
                    Some(key.clone())
                }
                _ => None,
            };
            args.push((key, value(&toks, &mut i)?));
            match toks.get(i) {
                Some(Tok::Punct(',')) => i += 1,
                Some(Tok::Punct(')')) => {}
                other => return Err(format!("expected `,` or `)`, found {other:?}")),
            }
        }
        i += 1;
        match name.as_str() {
            "prefix_rule" => parsed.rules.extend(prefix_rule(&args)?),
            "host_executable" => parsed.hosts.push(host_executable(&args)?),
            "network_rule" => parsed.network.push(network_rule(&args)?),
            other => return Err(format!("unknown function `{other}`")),
        }
    }
    Ok(parsed)
}

fn value(toks: &[Tok], i: &mut usize) -> Result<Val, String> {
    let tok = toks.get(*i).ok_or("unexpected end of file")?;
    *i += 1;
    match tok {
        Tok::Str(s) => Ok(Val::Str(s.clone())),
        Tok::Ident(s) => Ok(Val::Other(s.clone())),
        Tok::Punct('[') => {
            let mut items = Vec::new();
            while toks.get(*i) != Some(&Tok::Punct(']')) {
                items.push(value(toks, i)?);
                match toks.get(*i) {
                    Some(Tok::Punct(',')) => *i += 1,
                    Some(Tok::Punct(']')) => {}
                    other => return Err(format!("expected `,` or `]`, found {other:?}")),
                }
            }
            *i += 1;
            Ok(Val::List(items))
        }
        other => Err(format!("unexpected {other:?}")),
    }
}

fn strings(val: &Val) -> Option<Vec<String>> {
    match val {
        Val::Str(s) => Some(vec![s.clone()]),
        Val::List(items) => items
            .iter()
            .map(|v| match v {
                Val::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        Val::Other(_) => None,
    }
}

fn arg<'a>(args: &'a [(Option<String>, Val)], key: &str) -> Option<&'a Val> {
    args.iter()
        .find(|(k, _)| k.as_deref() == Some(key))
        .map(|(_, v)| v)
}

fn host_executable(args: &[(Option<String>, Val)]) -> Result<(String, Vec<String>), String> {
    let Some(Val::Str(name)) = arg(args, "name") else {
        return Err("host_executable needs a name".into());
    };
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(format!("host_executable name `{name}` must be a bare name"));
    }
    let paths = arg(args, "paths")
        .and_then(strings)
        .ok_or("host_executable needs paths")?;
    for path in &paths {
        let base = path.rsplit(['/', '\\']).next().unwrap_or_default();
        if !std::path::Path::new(path).is_absolute() || base != name {
            return Err(format!(
                "host_executable path `{path}` must have basename `{name}`"
            ));
        }
    }
    Ok((name.clone(), paths))
}

type ParsedNetwork = (NetworkRule, Decision, Option<String>, String);

fn network_rule(args: &[(Option<String>, Val)]) -> Result<ParsedNetwork, String> {
    let text = |key: &str| match arg(args, key) {
        Some(Val::Str(s)) => Ok(s.clone()),
        _ => Err(format!("network_rule needs `{key}`")),
    };
    let (host, protocol, decision) = (text("host")?, text("protocol")?, text("decision")?);
    let parsed = match decision.as_str() {
        "deny" | "forbidden" => Decision::Deny,
        "prompt" => Decision::Ask,
        "allow" => Decision::Allow,
        other => return Err(format!("unknown network_rule decision `{other}`")),
    };
    let justification = match arg(args, "justification") {
        Some(Val::Str(j)) if j.trim().is_empty() => {
            return Err("justification cannot be empty".into());
        }
        Some(Val::Str(j)) => Some(j.clone()),
        _ => None,
    };
    let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let shown =
        format!("network_rule(host=\"{host}\", protocol=\"{protocol}\", decision=\"{decision}\")");
    Ok((NetworkRule { host, protocol }, parsed, justification, shown))
}

/// One rule per alternative of the first token, as the Codex parser builds them.
pub fn expand(
    alts: &[Vec<String>],
    decision: Decision,
    justification: Option<String>,
    text: &str,
) -> Vec<CodexRule> {
    alts[0]
        .iter()
        .map(|head| {
            let mut pattern = vec![vec![head.clone()]];
            pattern.extend(alts[1..].iter().cloned());
            CodexRule {
                rule: PrefixRule {
                    pattern,
                    hosts: None,
                },
                decision,
                justification: justification.clone(),
                text: text.to_owned(),
            }
        })
        .collect()
}

fn prefix_rule(args: &[(Option<String>, Val)]) -> Result<Vec<CodexRule>, String> {
    let get = |key: &str| {
        args.iter()
            .find(|(k, _)| k.as_deref() == Some(key))
            .map(|(_, v)| v)
    };
    let pattern =
        get("pattern").or_else(|| args.first().filter(|(k, _)| k.is_none()).map(|(_, v)| v));
    let Some(Val::List(tokens)) = pattern else {
        return Err("prefix_rule needs a pattern list".into());
    };
    let alts: Vec<Vec<String>> = tokens
        .iter()
        .map(|t| strings(t).filter(|s| !s.is_empty()))
        .collect::<Option<_>>()
        .ok_or("pattern elements must be strings or non-empty lists of strings")?;
    if alts.is_empty() {
        return Err("pattern cannot be empty".into());
    }
    let decision = match get("decision") {
        None => Decision::Allow,
        Some(Val::Str(d)) => match d.as_str() {
            "allow" => Decision::Allow,
            "prompt" => Decision::Ask,
            "forbidden" => Decision::Deny,
            other => return Err(format!("unknown decision `{other}`")),
        },
        Some(_) => return Err("decision must be a string".into()),
    };
    let justification = match get("justification") {
        Some(Val::Str(j)) if j.trim().is_empty() => {
            return Err("justification cannot be empty".into());
        }
        Some(Val::Str(j)) => Some(j.clone()),
        _ => None,
    };
    let text = format!(
        "prefix_rule(pattern={}, decision=\"{}\")",
        show(tokens),
        match decision {
            Decision::Allow => "allow",
            Decision::Ask => "prompt",
            Decision::Deny => "forbidden",
        }
    );
    let rules = expand(&alts, decision, justification, &text);
    for (key, want) in [("match", true), ("not_match", false)] {
        let Some(Val::List(examples)) = get(key) else {
            continue;
        };
        for example in examples {
            let words = match example {
                Val::Str(s) => shell::parse(s)
                    .commands
                    .first()
                    .map(|c| shell::values(c))
                    .unwrap_or_default(),
                other => strings(other).ok_or("examples must be strings or lists of strings")?,
            };
            if rules.iter().any(|r| r.rule.matches(&words)) != want {
                return Err(format!("{text}: {key} example {words:?} fails"));
            }
        }
    }
    Ok(rules)
}

fn show(tokens: &[Val]) -> String {
    let parts: Vec<String> = tokens
        .iter()
        .map(|t| match t {
            Val::Str(s) => format!("{s:?}"),
            Val::List(items) => show(items),
            Val::Other(s) => s.clone(),
        })
        .collect();
    format!("[{}]", parts.join(", "))
}

fn tokenize(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut toks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let raw = matches!(c, 'r' | 'R') && matches!(chars.get(i + 1), Some('"' | '\''));
        if raw || c == '"' || c == '\'' {
            i += usize::from(raw);
            toks.push(Tok::Str(string(&chars, &mut i, raw)?));
        } else if c.is_whitespace() {
            i += 1;
        } else if c == '#' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c.is_ascii_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            toks.push(Tok::Ident(chars[start..i].iter().collect()));
        } else if "()[],=".contains(c) {
            toks.push(Tok::Punct(c));
            i += 1;
        } else {
            return Err(format!("unexpected character `{c}`"));
        }
    }
    Ok(toks)
}

fn string(chars: &[char], i: &mut usize, raw: bool) -> Result<String, String> {
    let quote = chars[*i];
    let triple = chars.get(*i + 1) == Some(&quote) && chars.get(*i + 2) == Some(&quote);
    *i += if triple { 3 } else { 1 };
    let mut out = String::new();
    loop {
        let c = *chars.get(*i).ok_or("unterminated string")?;
        if c == quote
            && (!triple || (chars.get(*i + 1) == Some(&quote) && chars.get(*i + 2) == Some(&quote)))
        {
            *i += if triple { 3 } else { 1 };
            return Ok(out);
        }
        if c == '\n' && !triple {
            return Err("unterminated string".into());
        }
        *i += 1;
        if c == '\\' && !raw {
            let next = *chars.get(*i).ok_or("unterminated string")?;
            *i += 1;
            match next {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'r' => out.push('\r'),
                '\\' | '\'' | '"' => out.push(next),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
        } else {
            out.push(c);
        }
    }
}

#[cfg(test)]
#[path = "codex_tests.rs"]
mod tests;
