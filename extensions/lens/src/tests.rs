use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::json;

use crate::pipeline::Runner;
use crate::state::{Guard, small_text};
use crate::{config, detection, diagnostics};

static NEXT: AtomicU64 = AtomicU64::new(0);

pub(crate) struct Project(pub PathBuf);

impl Project {
    pub fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "pi-lens-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    pub fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn merges_config_and_rejects_untrusted_commands() {
    let project = Project::new();
    let agent = project.0.join("agent");
    project.write("agent/ext/lens.json", r#"{"context":false,"tools":{"ruff":{"enabled":false}},"commands":[{"name":"global","language":"python","stage":"lint","command":"ruff"}]}"#);
    project.write(".pi-lens.json", r#"{"timeoutMs":7000}"#);
    project.write(
        ".pi/ext/lens.json",
        r#"{"context":true,"tools":{"tsc":{"enabled":false}},"commands":[],"lsp":{"servers":[]}}"#,
    );
    let (config, ignored) = config::load(&agent, &project.0, false).unwrap();
    assert!(ignored);
    assert!(config.context);
    assert!(!config.tool_enabled("ruff"));
    assert!(!config.tool_enabled("tsc"));
    assert_eq!(config.timeout_ms, 7000);
    assert_eq!(config.commands[0].name, "global");
    let (config, ignored) = config::load(&agent, &project.0, true).unwrap();
    assert!(!ignored);
    assert!(config.commands.is_empty());
    assert!(serde_json::from_value::<config::ToolCommand>(json!({"name":"bad"})).is_err());
}

#[test]
fn guard_tracks_partial_reads_and_external_changes() {
    let project = Project::new();
    let path = project.write("a.py", "first\nsecond\nthird\n");
    let mut guard = Guard::default();
    assert!(
        guard
            .block_reason(
                &path,
                small_text(&path),
                "edit",
                &json!({"oldText":"first"})
            )
            .is_some()
    );
    guard.record(
        &path,
        small_text(&path),
        &json!({"offset":2,"limit":1}),
        &[json!({"text":"second"})],
    );
    assert!(
        guard
            .block_reason(
                &path,
                small_text(&path),
                "edit",
                &json!({"oldText":"second"})
            )
            .is_none()
    );
    assert!(
        guard
            .block_reason(
                &path,
                small_text(&path),
                "edit",
                &json!({"oldText":"first"})
            )
            .is_some()
    );
    assert!(
        guard
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_some()
    );
    guard.record(
        &path,
        small_text(&path),
        &json!({}),
        &[json!({"text":"first\nsecond\nthird"})],
    );
    assert!(
        guard
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_none()
    );
    project.write("a.py", "changed\n");
    assert!(
        guard
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_some()
    );
    guard.allowed.insert(path.clone());
    assert!(
        guard
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_none()
    );
    assert!(
        guard
            .block_reason(&path, small_text(&path), "write", &json!({}))
            .is_some()
    );
    assert!(
        guard
            .block_reason(&project.0.join("new.py"), None, "write", &json!({}))
            .is_none()
    );
}

#[test]
fn parses_supported_diagnostic_formats() {
    let root = std::path::Path::new("/work");
    let file = root.join("a.rs");
    let cargo = r#"{"reason":"compiler-message","message":{"message":"wrong type","level":"error","spans":[{"is_primary":true,"file_name":"a.rs","line_start":4,"column_start":2}]}}"#;
    assert_eq!(
        diagnostics::parse("cargo", cargo, root, &file)[0]["line"],
        4
    );
    let ruff =
        r#"[{"filename":"a.py","message":"unused","location":{"row":3,"column":1},"code":"F401"}]"#;
    assert_eq!(
        diagnostics::parse("ruff", ruff, root, &file)[0]["code"],
        "F401"
    );
    let eslint = r#"[{"filePath":"a.ts","messages":[{"line":2,"column":4,"message":"missing","severity":2}]}]"#;
    assert_eq!(
        diagnostics::parse("eslint", eslint, root, &file)[0]["severity"],
        "error"
    );
    assert_eq!(
        diagnostics::parse("tsc", "a.ts(2,4): error TS123: missing", root, &file)[0]["column"],
        4
    );
    assert_eq!(
        diagnostics::parse("go", "a.go:2:4: wrong", root, &file)[0]["path"],
        "/work/a.go"
    );
}

#[test]
fn detects_project_tools_and_roots() {
    let project = Project::new();
    project.write(
        "package.json",
        r#"{"devDependencies":{"eslint":"1","prettier":"1"}}"#,
    );
    project.write("tsconfig.json", "{}");
    let file = project.write("src/a.ts", "");
    let root = detection::root(&file, &project.0, "javascript");
    assert_eq!(root, project.0);
    let names: Vec<_> = detection::detect(&root, "javascript")
        .iter()
        .map(|tool| tool.name.clone())
        .collect();
    assert_eq!(names, ["prettier", "eslint", "tsc"]);
    project.write("Cargo.toml", "[package]");
    assert_eq!(detection::detect(&root, "rust").len(), 3);
    project.write("go.mod", "module test");
    assert_eq!(detection::detect(&root, "go").len(), 2);
}

#[cfg(unix)]
pub(crate) fn executable(project: &Project, name: &str, text: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = project.write(name, text);
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    path
}

#[cfg(unix)]
#[tokio::test]
async fn fake_pipeline_formats_then_lints_and_clears_findings() {
    let project = Project::new();
    project.write("pyproject.toml", "");
    let file = project.write("a.py", "before\n");
    executable(
        &project,
        "node_modules/.bin/ruff",
        "#!/bin/sh\nif [ \"$1\" = format ]; then\n  printf 'formatted\\n' > \"$2\"\n  printf 'format\\n' >> order\nelse\n  printf 'lint\\n' >> order\n  cat diagnostics.json\nfi\n",
    );
    project.write(
        "diagnostics.json",
        r#"[{"filename":"a.py","message":"unused","location":{"row":1,"column":1},"code":"F401"}]"#,
    );
    let config = config::Config {
        trusted: true,
        ..Default::default()
    };
    let findings = crate::pipeline::run(&config, &project.0, &file, &Runner::default()).await;
    assert_eq!(findings.len(), 1, "{findings:?}");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "formatted\n");
    assert_eq!(
        std::fs::read_to_string(project.0.join("order")).unwrap(),
        "format\nlint\n"
    );
    project.write("diagnostics.json", "[]");
    assert!(
        crate::pipeline::run(&config, &project.0, &file, &Runner::default())
            .await
            .is_empty()
    );
    let mut config = config;
    config
        .languages
        .insert("python".to_owned(), config::Toggle { enabled: false });
    project.write("order", "");
    assert!(
        crate::pipeline::run(&config, &project.0, &file, &Runner::default())
            .await
            .is_empty()
    );
    assert_eq!(
        std::fs::read_to_string(project.0.join("order")).unwrap(),
        ""
    );
}

#[cfg(unix)]
#[tokio::test]
async fn timeout_becomes_a_finding() {
    let project = Project::new();
    let file = project.write("a.py", "");
    let slow = executable(&project, "slow", "#!/bin/sh\nexec sleep 10\n");
    let mut config = config::Config {
        auto_detect: false,
        timeout_ms: 30,
        ..Default::default()
    };
    config.commands.push(config::ToolCommand {
        name: "slow".into(),
        language: "python".into(),
        stage: config::Stage::Lint,
        command: slow.to_string_lossy().into_owned(),
        args: vec![],
        enabled: true,
    });
    let findings = crate::pipeline::run(&config, &project.0, &file, &Runner::default()).await;
    assert!(
        findings[0]["message"]
            .as_str()
            .unwrap()
            .contains("timed out")
    );
}

#[path = "protocol_tests.rs"]
mod protocol;

#[path = "trust_tests.rs"]
mod trust;

#[path = "hang_tests.rs"]
mod hang;
