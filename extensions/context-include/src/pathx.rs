//! POSIX path helpers with Node `path` semantics, on strings.

use std::path::Path;

pub fn is_absolute(p: &str) -> bool {
    p.starts_with('/')
}

/// Lexical `path.resolve(base, p)`: `.` and `..` removed, no trailing slash.
pub fn resolve(base: &str, p: &str) -> String {
    let joined = if is_absolute(p) {
        p.to_owned()
    } else {
        format!("{base}/{p}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `path.join(home, rest)` for a `~` prefix; other paths unchanged.
pub fn expand_tilde(p: &str, home: &str) -> String {
    match p.strip_prefix('~') {
        Some(rest) => resolve(home, rest.trim_start_matches('/')),
        None => p.to_owned(),
    }
}

pub fn dirname(p: &str) -> String {
    match p.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(i) => p[..i].to_owned(),
        None => ".".to_owned(),
    }
}

/// `path.relative(from, to)` for absolute, normalized paths.
pub fn relative(from: &str, to: &str) -> String {
    let from: Vec<&str> = from.split('/').filter(|s| !s.is_empty()).collect();
    let to: Vec<&str> = to.split('/').filter(|s| !s.is_empty()).collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out: Vec<&str> = vec![".."; from.len() - common];
    out.extend(&to[common..]);
    out.join("/")
}

/// `realpath`, falling back to the lexical path when it fails.
pub fn canonicalize(p: &str, cwd: &str) -> String {
    let abs = resolve(cwd, p);
    match std::fs::canonicalize(Path::new(&abs)) {
        Ok(real) => real.to_string_lossy().into_owned(),
        Err(_) => abs,
    }
}

fn to_posix(p: &str) -> String {
    p.replace('\\', "/")
}

fn has_drive(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'/'
}

/// True when `target` equals `root` or sits below it.
pub fn under_root(target: &str, root: &str) -> bool {
    let (t, r) = (to_posix(target), to_posix(root));
    let t = if has_drive(&t) { t.to_lowercase() } else { t };
    let r = if has_drive(&r) { r.to_lowercase() } else { r };
    if t == r {
        return true;
    }
    let prefix = if r.ends_with('/') { r } else { format!("{r}/") };
    t.starts_with(&prefix)
}

pub fn under_any_root(target: &str, roots: &[String]) -> bool {
    roots.iter().any(|root| under_root(target, root))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_like_node() {
        assert_eq!(resolve("/a/b", "../c/./d.md"), "/a/c/d.md");
        assert_eq!(resolve("/a", "/x/y/"), "/x/y");
        assert_eq!(resolve("/", ".."), "/");
        assert_eq!(
            expand_tilde("~/.agents/x.md", "/home/u"),
            "/home/u/.agents/x.md"
        );
        assert_eq!(expand_tilde("~", "/home/u"), "/home/u");
        assert_eq!(dirname("/a/b.md"), "/a");
        assert_eq!(dirname("/a"), "/");
        assert_eq!(relative("/w", "/w/a/b.md"), "a/b.md");
        assert_eq!(relative("/w/x", "/w/a.md"), "../a.md");
        assert_eq!(relative("/w", "/w"), "");
    }

    #[test]
    fn root_prefix_uses_separator_boundary() {
        assert!(under_root("/a/b", "/a"));
        assert!(under_root("/a", "/a"));
        assert!(under_root("/a/b", "/a/"));
        assert!(!under_root("/ab", "/a"));
        assert!(under_root("C:/X/y", "c:/x"));
        assert!(!under_root("/A/y", "/a"));
    }
}
