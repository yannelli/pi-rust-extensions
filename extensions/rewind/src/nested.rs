use std::path::Path;

const MAX_DEPTH: usize = 8;
const MAX_DIRS: usize = 20_000;

/// `/<dir>/` exclude lines for git repos below `root`, skipping excluded directory names.
pub fn nested_repos(root: &Path, patterns: &[String]) -> Vec<String> {
    let skipped: Vec<&str> = patterns
        .iter()
        .filter_map(|pattern| pattern.strip_suffix('/'))
        .filter(|name| !name.contains(['/', '*', '?', '[', '!']))
        .collect();
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > MAX_DIRS {
            break;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if !kind.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name == ".git" || skipped.contains(&name.as_ref()) {
                continue;
            }
            let path = entry.path();
            if path.join(".git").exists() {
                if let Ok(relative) = path.strip_prefix(root) {
                    found.push(format!("/{}/", relative.to_string_lossy()));
                }
            } else if depth < MAX_DEPTH {
                stack.push((path, depth + 1));
            }
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testdir::TestDir;

    #[test]
    fn finds_nested_repos_and_skips_excluded_dirs() {
        let dir = TestDir::new("nested");
        let root = dir.path();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("libs/inner/.git")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/pkg/.git")).unwrap();
        std::fs::write(root.join("vendor.git"), "").unwrap();
        let patterns = vec!["node_modules/".to_string()];
        assert_eq!(nested_repos(root, &patterns), ["/libs/inner/"]);
    }
}
