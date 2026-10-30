use std::fs;
use std::path::Path;

use pi_ext_context_include::config::Config;
use pi_ext_context_include::fmt::status_text;
use pi_ext_context_include::scan::{ContextFile, Scanner, extract_references};

struct Fixture {
    _dir: tempfile::TempDir,
    root: String,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap().display().to_string();
        for sub in ["home", "agent/includes", "proj"] {
            fs::create_dir_all(format!("{root}/{sub}")).unwrap();
        }
        Self { _dir: dir, root }
    }

    fn p(&self, rel: &str) -> String {
        format!("{}/{rel}", self.root)
    }

    fn write(&self, rel: &str, content: &str) -> String {
        let path = self.p(rel);
        fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn scanner(&self, config: Config) -> Scanner {
        Scanner {
            home: self.p("home"),
            agent_dir: self.p("agent"),
            cwd: self.p("proj"),
            config,
        }
    }

    fn agents(&self, content: &str) -> Vec<ContextFile> {
        let path = self.write("proj/AGENTS.md", content);
        vec![ContextFile {
            path: Some(path),
            content: content.into(),
        }]
    }
}

fn reasons(diag: &pi_ext_context_include::scan::Diagnostic) -> Vec<(String, String)> {
    diag.skipped
        .iter()
        .map(|s| (s.reference.clone(), s.reason.clone()))
        .collect()
}

#[test]
fn extracts_references_like_upstream() {
    let text = "# T\n@a.md\n- @b.md\n* @c.md\n+ @d.md\n12. @e.md\n-@f.md\n  @~/g.md  \n@./h.md\n\
                ```\n@fenced.md\n```\n@a.md\nsee @x.md\n@x.txt\n@Dockerfile\n@.md\n@~.md\n@sp ace.md\n@../up.md\r";
    assert_eq!(
        extract_references(text),
        [
            "a.md", "b.md", "c.md", "d.md", "e.md", "~/g.md", "./h.md", "../up.md"
        ]
    );
}

#[test]
fn includes_recursively_with_output_format() {
    let f = Fixture::new();
    f.write("proj/docs/a.md", "A\n@../b.md");
    f.write("proj/b.md", "B");
    let out = f.scanner(Config::default()).scan(&f.agents("@docs/a.md"));
    let expected = format!(
        "Additional files included via @-syntax references in the project instructions:\n\n\
         <project_instructions path=\"{0}/proj/docs/a.md\">\nA\n@../b.md\n</project_instructions>\n\n\
         <project_instructions path=\"{0}/proj/b.md\">\nB\n</project_instructions>",
        f.root
    );
    assert_eq!(out.section.as_deref(), Some(expected.as_str()));
    assert_eq!(out.diag.total_bytes, 11);
    assert_eq!(out.diag.included[1], (f.p("proj/b.md"), 1));
}

#[test]
fn dedupes_cycles_and_symlinks() {
    let f = Fixture::new();
    f.write("proj/a.md", "@b.md\n@AGENTS.md");
    f.write("proj/b.md", "@a.md");
    std::os::unix::fs::symlink(f.p("proj/a.md"), f.p("proj/link.md")).unwrap();
    let out = f
        .scanner(Config::default())
        .scan(&f.agents("@a.md\n@link.md"));
    assert_eq!(out.diag.included.len(), 2);
    assert_eq!(
        reasons(&out.diag),
        [
            ("a.md".into(), "already included".into()),
            ("AGENTS.md".into(), "already included".into()),
            ("link.md".into(), "already included".into()),
        ]
    );
}

#[test]
fn relative_refs_resolve_from_realpath_dir() {
    let f = Fixture::new();
    f.write("proj/real/inner.md", "@sib.md");
    f.write("proj/real/sib.md", "S");
    fs::create_dir_all(f.p("proj/alias")).unwrap();
    std::os::unix::fs::symlink(f.p("proj/real/inner.md"), f.p("proj/alias/inner.md")).unwrap();
    let out = f
        .scanner(Config::default())
        .scan(&f.agents("@alias/inner.md"));
    assert_eq!(out.diag.included[0].0, f.p("proj/alias/inner.md"));
    assert_eq!(out.diag.included[1].0, f.p("proj/real/sib.md"));
}

#[test]
fn tilde_uses_home_and_default_roots() {
    let f = Fixture::new();
    f.write("home/x.md", "X");
    f.write("agent/includes/shared.md", "S");
    f.write("other/o.md", "O");
    let refs = format!(
        "@~/x.md\n@{}\n@../other/o.md",
        f.p("agent/includes/shared.md")
    );
    let scanner = f.scanner(Config::default());
    let out = scanner.scan(&f.agents(&refs));
    assert_eq!(out.diag.allowed_roots, [f.p("agent/includes"), f.p("proj")]);
    assert_eq!(out.diag.included, [(f.p("agent/includes/shared.md"), 1)]);
    assert_eq!(
        reasons(&out.diag),
        [
            ("~/x.md".into(), "outside allowed roots".into()),
            ("../other/o.md".into(), "outside allowed roots".into()),
        ]
    );

    let config = Config {
        allowed_roots: Some(vec!["~".into()]),
        ..Config::default()
    };
    let out = f.scanner(config).scan(&f.agents("@~/x.md"));
    assert_eq!(out.diag.included, [(f.p("home/x.md"), 1)]);
}

#[test]
fn deny_wins_and_empty_allow_blocks_all() {
    let f = Fixture::new();
    f.write("proj/secret/s.md", "S");
    f.write("proj/ok.md", "K");
    f.write("lib/l.md", "L");
    let config = Config {
        allowed_roots: Some(vec![".".into(), "../lib".into()]),
        denied_roots: vec!["secret".into()],
        ..Config::default()
    };
    let out = f
        .scanner(config)
        .scan(&f.agents("@secret/s.md\n@ok.md\n@../lib/l.md"));
    assert_eq!(out.diag.denied_roots, [f.p("proj/secret")]);
    assert_eq!(out.diag.included.len(), 2);
    assert_eq!(
        reasons(&out.diag),
        [("secret/s.md".into(), "denied by deniedRoots".into())]
    );

    let config = Config {
        allowed_roots: Some(vec![]),
        ..Config::default()
    };
    let out = f.scanner(config).scan(&f.agents("@ok.md"));
    assert_eq!(out.section, None);
    assert_eq!(
        reasons(&out.diag),
        [("ok.md".into(), "outside allowed roots".into())]
    );
}

#[test]
fn size_limit_is_a_running_total() {
    let f = Fixture::new();
    f.write("proj/a.md", "aaaa");
    f.write("proj/big.md", "bbbbbbb");
    f.write("proj/c.md", "cc");
    let config = Config {
        max_bytes: 6.0,
        ..Config::default()
    };
    let out = f.scanner(config).scan(&f.agents("@a.md\n@big.md\n@c.md"));
    assert_eq!(
        out.diag.included,
        [(f.p("proj/a.md"), 4), (f.p("proj/c.md"), 2)]
    );
    assert_eq!(
        reasons(&out.diag),
        [(
            "big.md".into(),
            "size limit exceeded (would reach 11 bytes)".into()
        )]
    );
    let status = status_text(10.0, 6.0, Some(&out.diag));
    assert!(status.contains("**Included** (2 files, 6 B)"), "{status}");
    assert!(
        status.contains("⚠️  Total (6 B) is at or above the 6 B limit"),
        "{status}"
    );
}

#[test]
fn max_depth_stops_recursion() {
    let f = Fixture::new();
    f.write("proj/one.md", "@two.md");
    f.write("proj/two.md", "@three.md");
    f.write("proj/three.md", "3");
    let config = Config {
        max_depth: 1.0,
        ..Config::default()
    };
    let out = f.scanner(config).scan(&f.agents("@one.md"));
    assert_eq!(out.diag.included.len(), 2);
    assert_eq!(
        reasons(&out.diag),
        [("two.md".into(), "max depth (1) exceeded".into())]
    );
    assert_eq!(out.diag.skipped[0].resolved_path, f.p("proj/two.md"));
}

#[test]
fn empty_missing_and_unreadable_files() {
    let f = Fixture::new();
    f.write("proj/empty.md", "");
    fs::create_dir_all(f.p("proj/dir.md")).unwrap();
    let out = f
        .scanner(Config::default())
        .scan(&f.agents("@empty.md\n@gone.md\n@dir.md"));
    assert_eq!(out.section, None);
    let r = reasons(&out.diag);
    assert_eq!(r[0], ("empty.md".into(), "file is empty".into()));
    assert_eq!(r[1], ("gone.md".into(), "file not found".into()));
    assert!(r[2].1.starts_with("read error: "), "{:?}", r[2]);
}

#[test]
fn context_file_status_lines() {
    let f = Fixture::new();
    f.write("proj/inc.md", "I");
    let mut files = f.agents("@inc.md");
    files.push(ContextFile {
        path: Some(f.p("proj/missing/AGENTS.md")),
        content: "@inc.md".into(),
    });
    files.push(ContextFile {
        path: None,
        content: String::new(),
    });
    let out = f.scanner(Config::default()).scan(&files);
    let text = status_text(10.0, 500_000.0, Some(&out.diag));
    let expected = format!(
        "**pi-context-include**\n\nmaxDepth: 10\nmaxBytes: 500,000 (488 KB)\n\n\
         **Allowed roots** (3)\n  {0}/agent/includes\n  {0}/proj\n  {0}/proj/missing\n\n\
         **Context files** (3)\n  ok  {0}/proj/AGENTS.md\n  not found  {0}/proj/missing/AGENTS.md\n  ERROR: empty path  <empty>\n\n\
         **Included** (1 files, 1 B)\n  1 B  {0}/proj/inc.md",
        f.root
    );
    assert_eq!(text, expected);
}
