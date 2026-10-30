//! Claude rule patterns: Bash wildcards, gitignore-style paths, and `domain:` hosts.

use std::path::{Component, Path, PathBuf};

use regex::Regex;

/// `Bash(...)` specifier against one command. `*` matches any text; a trailing ` *` (or `:*`)
/// that is the only wildcard also matches the bare command.
pub fn bash_matches(pattern: &str, command: &str) -> bool {
    let pattern = match pattern.strip_suffix(":*") {
        Some(head) => format!("{head} *"),
        None => pattern.to_owned(),
    };
    if pattern.matches('*').count() == 1
        && let Some(bare) = pattern.strip_suffix(" *")
        && command == bare
    {
        return true;
    }
    let body: Vec<String> = pattern.split('*').map(regex::escape).collect();
    Regex::new(&format!("(?s)^{}$", body.join(".*"))).is_ok_and(|re| re.is_match(command))
}

/// Tool-name glob such as `mcp__*` or `mcp__github__get_*`.
pub fn name_glob(pattern: &str, name: &str) -> bool {
    let body: Vec<String> = pattern.split('*').map(regex::escape).collect();
    Regex::new(&format!("^{}$", body.join(".*"))).is_ok_and(|re| re.is_match(name))
}

pub struct PathContext<'a> {
    pub cwd: &'a Path,
    pub home: &'a Path,
    /// Where `/path` patterns anchor for this rule's file.
    pub anchor: &'a Path,
}

/// `Read(...)`/`Edit(...)` specifier against an absolute path, with gitignore semantics.
/// `any_depth_dirs` lets a relative `dir/**` match at any depth (deny and ask rules).
pub fn path_matches(pattern: &str, cx: &PathContext, target: &Path, any_depth_dirs: bool) -> bool {
    let (base, rest, relative) = if let Some(rest) = pattern.strip_prefix("//") {
        (PathBuf::from("/"), rest, false)
    } else if let Some(rest) = pattern.strip_prefix("~/") {
        (cx.home.to_path_buf(), rest, false)
    } else if let Some(rest) = pattern.strip_prefix('/') {
        (cx.anchor.to_path_buf(), rest, false)
    } else {
        let rest = pattern.strip_prefix("./").unwrap_or(pattern);
        (cx.cwd.to_path_buf(), rest, true)
    };
    let rest = rest.trim_end_matches('/');
    if rest.is_empty() {
        return normalize(target).starts_with(normalize(&base));
    }
    let Ok(rel) = normalize(target)
        .strip_prefix(normalize(&base))
        .map(Path::to_path_buf)
    else {
        return false;
    };
    let segments: Vec<&str> = rest.split('/').collect();
    let single_dir = segments.len() == 2 && segments[1] == "**" && !segments[0].contains('*');
    let any_depth = relative && (segments.len() == 1 || (single_dir && any_depth_dirs));
    let Some(re) = glob_regex(rest, any_depth) else {
        return false;
    };
    let parts: Vec<String> = rel
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    (1..=parts.len()).any(|n| re.is_match(&parts[..n].join("/")))
}

fn glob_regex(glob: &str, any_depth: bool) -> Option<Regex> {
    let mut out = String::from(if any_depth { "^(?:.*/)?" } else { "^" });
    let chars: Vec<char> = glob.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' if chars.get(i + 1) == Some(&'*') => {
                let slash = chars.get(i + 2) == Some(&'/');
                out.push_str(if slash { "(?:.*/)?" } else { ".*" });
                i += if slash { 3 } else { 2 };
                continue;
            }
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            c => out.push_str(&regex::escape(&c.to_string())),
        }
        i += 1;
    }
    out.push('$');
    Regex::new(&out).ok()
}

/// Lexical `.`/`..` removal.
pub fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// `domain:` specifier against a host: `*` is every host, `*.x` any subdomain depth,
/// another `*` one label.
pub fn domain_matches(pattern: &str, host: &str) -> bool {
    let pattern = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix("*.") {
        return host.ends_with(&format!(".{suffix}"));
    }
    let body: Vec<String> = pattern.split('*').map(regex::escape).collect();
    Regex::new(&format!("^{}$", body.join("[^.]*"))).is_ok_and(|re| re.is_match(&host))
}

/// The host of an `http(s)` URL.
pub fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?;
    let host = if let Some(v6) = host.strip_prefix('[') {
        v6.split(']').next()?
    } else {
        host.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

#[cfg(test)]
#[path = "pattern_tests.rs"]
mod tests;
