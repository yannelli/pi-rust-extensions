use super::*;

#[test]
fn bash_wildcards_follow_claude_docs() {
    assert!(bash_matches("npm run build", "npm run build"));
    assert!(!bash_matches("npm run build", "npm run build --watch"));
    assert!(bash_matches("npm run *", "npm run test --watch"));
    assert!(bash_matches("npm run *", "npm run"));
    assert!(!bash_matches("npm run *", "npm install"));
    assert!(bash_matches("git log * main", "git log --oneline main"));
    assert!(!bash_matches("git log * main", "git log main"));
    assert!(bash_matches("* --version", "node --version"));
    assert!(bash_matches("ls *", "ls"));
    assert!(!bash_matches("ls *", "lsof"));
    assert!(bash_matches("ls*", "lsof"));
    assert!(bash_matches("* --help *", "npm --help x"));
    assert!(!bash_matches("* --help *", "npm --help"));
    assert!(bash_matches("npm run test:*", "npm run test"));
    assert!(bash_matches("ls:*", "ls -la"));
    assert!(!bash_matches("git:* push", "git push"));
}

#[test]
fn tool_name_globs() {
    assert!(name_glob("mcp__*", "mcp__fs__read"));
    assert!(name_glob("mcp__github__get_*", "mcp__github__get_issue"));
    assert!(!name_glob("mcp__github__get_*", "mcp__github__list"));
}

fn cx<'a>(cwd: &'a Path, home: &'a Path, anchor: &'a Path) -> PathContext<'a> {
    PathContext { cwd, home, anchor }
}

#[test]
fn path_anchors() {
    let (cwd, home) = (Path::new("/w/proj"), Path::new("/home/u"));
    let c = cx(cwd, home, Path::new("/home/u/.claude"));
    let p = |s: &str| PathBuf::from(s);
    assert!(path_matches(
        "//tmp/scratch.txt",
        &c,
        &p("/tmp/scratch.txt"),
        true
    ));
    assert!(path_matches("~/.zshrc", &c, &p("/home/u/.zshrc"), true));
    assert!(path_matches(
        "/secrets/**",
        &c,
        &p("/home/u/.claude/secrets/a"),
        true
    ));
    assert!(!path_matches(
        "/secrets/**",
        &c,
        &p("/w/proj/secrets/a"),
        true
    ));
    assert!(path_matches("./.env", &c, &p("/w/proj/.env"), true));
    assert!(path_matches(".env", &c, &p("/w/proj/a/b/.env"), true));
    assert!(path_matches("**/.env", &c, &p("/w/proj/a/.env"), true));
    assert!(!path_matches(".env", &c, &p("/w/.env"), true));
    assert!(path_matches("*.env", &c, &p("/w/proj/x/prod.env"), true));
    assert!(path_matches("//**/.env", &c, &p("/anywhere/.env"), true));
}

#[test]
fn path_depth_by_rule_type() {
    let (cwd, home) = (Path::new("/w"), Path::new("/h"));
    let c = cx(cwd, home, cwd);
    let top = PathBuf::from("/w/src/app.ts");
    let nested = PathBuf::from("/w/vendor/pkg/src/lib.js");
    assert!(path_matches("src/**", &c, &top, false));
    assert!(!path_matches("src/**", &c, &nested, false));
    assert!(path_matches("src/**", &c, &nested, true));
    assert!(!path_matches("/src/**", &c, &nested, true));
    assert!(path_matches("**/src/**", &c, &nested, false));
    assert!(path_matches("src/**/*.ts", &c, &top, false));
    assert!(!path_matches("src/*.js", &c, &top, false));
    assert!(path_matches(
        "./secrets",
        &c,
        &PathBuf::from("/w/secrets/key"),
        true
    ));
}

#[test]
fn domains() {
    assert!(domain_matches("example.com", "EXAMPLE.com."));
    assert!(!domain_matches("example.com", "api.example.com"));
    assert!(domain_matches("*.example.com", "a.b.example.com"));
    assert!(!domain_matches("*.example.com", "example.com"));
    assert!(domain_matches("example.*", "example.org"));
    assert!(!domain_matches("example.*", "example.evil.com"));
    assert!(domain_matches("*", "x.y"));
    assert_eq!(
        url_host("https://user@API.Example.com:8443/p?q").as_deref(),
        Some("api.example.com")
    );
    assert_eq!(url_host("http://[::1]:80/").as_deref(), Some("::1"));
}
