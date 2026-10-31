use super::*;

fn texts(script: &str) -> Vec<String> {
    parse(script).commands.iter().map(|c| raw_text(c)).collect()
}

#[test]
fn splits_compound_commands() {
    assert_eq!(
        texts("git status && npm test; ls -la | grep x || echo 'a b'"),
        ["git status", "npm test", "ls -la", "grep x", "echo 'a b'"]
    );
    assert_eq!(texts("a |& b & c\nd"), ["a", "b", "c", "d"]);
    let s = parse("git status && npm test | wc -l");
    assert!(s.plain && s.complete);
}

#[test]
fn quotes_and_values() {
    let s = parse(r#"echo "a && b" 'c;d' e\ f"#);
    assert_eq!(s.commands.len(), 1);
    assert_eq!(values(&s.commands[0]), ["echo", "a && b", "c;d", "e f"]);
    assert!(s.plain);
}

#[test]
fn nested_commands_are_found() {
    assert_eq!(
        texts(r#"echo "$(git clean -f)" && (cd /tmp; rm -rf x) && echo `whoami`"#),
        [
            "git clean -f",
            "echo \"$(git clean -f)\"",
            "cd /tmp",
            "rm -rf x",
            "whoami",
            "echo `whoami`"
        ]
    );
    assert!(!parse("echo $(ls)").plain);
}

#[test]
fn redirections_and_not_plain_scripts() {
    let s = parse("echo hi > out.txt 2>&1");
    assert_eq!(texts("echo hi > out.txt 2>&1"), ["echo hi"]);
    assert!(!s.plain);
    assert!(!parse("ls *.rs").plain);
    assert!(!parse("FOO=1 make").plain);
    assert!(!parse("echo $HOME").plain);
    assert_eq!(
        texts("if grep -q x f; then rm f; fi"),
        ["grep -q x f", "rm f"]
    );
    assert_eq!(texts("for f in a b; do rm $f; done"), ["rm $f"]);
}

#[test]
fn trailing_operator_is_incomplete() {
    assert!(!parse("npm test &&").complete);
    assert!(!parse("npm test |").complete);
    assert!(parse("npm test;").complete);
    assert!(parse("sleep 1 &").complete);
}

#[test]
fn strips_wrappers() {
    let words = |s: &str| parse(s).commands.remove(0);
    let w = words("timeout 30 nice -n 5 nohup npm test");
    assert_eq!(raw_text(strip_wrappers(&w, false)), "npm test");
    let w = words("NODE_ENV=test npm test");
    assert_eq!(raw_text(strip_wrappers(&w, true)), "npm test");
    assert_eq!(
        raw_text(strip_wrappers(&w, false)),
        "NODE_ENV=test npm test"
    );
    let w = words("xargs grep x");
    assert_eq!(raw_text(strip_wrappers(&w, false)), "grep x");
    let w = words("xargs -n1 grep x");
    assert_eq!(raw_text(strip_wrappers(&w, false)), "xargs -n1 grep x");
    let w = words("command -v git");
    assert_eq!(raw_text(strip_wrappers(&w, false)), "command -v git");
}

#[test]
fn heredoc_bodies_are_not_commands() {
    let script = "cat <<EOF > notes.txt\nrm -rf /\ngit push --force\nEOF\nls";
    assert_eq!(texts(script), ["cat", "ls"]);
    let tabs = "cat <<-'END'\n\trm -rf x\n\tEND\necho done";
    assert_eq!(texts(tabs), ["cat", "echo done"]);
    let s = parse("python3 - <<'PY'\nimport os; os.remove('x')\nPY");
    assert_eq!(s.commands.len(), 1);
}

#[test]
fn expanding_heredocs_run_their_substitutions() {
    let script = "cat <<EOF\nvalue: $(rm -rf build)\n`whoami`\nEOF";
    assert_eq!(texts(script), ["cat", "rm -rf build", "whoami"]);
    let quoted = "cat <<'EOF'\n$(rm -rf build)\nEOF";
    assert_eq!(texts(quoted), ["cat"]);
}

#[test]
fn redirect_targets_are_collected() {
    let s = parse("sort < in.txt > out.txt 2>> err.log; echo x &> all.log; cmd 2>&1 >/dev/null");
    let targets: Vec<(&str, bool)> = s
        .redirects
        .iter()
        .map(|r| (r.path.as_str(), r.write))
        .collect();
    assert_eq!(
        targets,
        [
            ("in.txt", false),
            ("out.txt", true),
            ("err.log", true),
            ("all.log", true)
        ]
    );
    assert!(
        parse("cat <<< 'text' >| clobber.txt")
            .redirects
            .iter()
            .any(|r| r.path == "clobber.txt")
    );
    assert!(parse("cat <<< 'secret.txt'").redirects.is_empty());
}
