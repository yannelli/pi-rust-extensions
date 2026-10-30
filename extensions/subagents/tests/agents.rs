use std::path::Path;

use pi_ext_subagents::agent::{PromptMode, Source, map_tools};
use pi_ext_subagents::frontmatter::{FmValue, Frontmatter};
use pi_ext_subagents::registry::{self, Layers, merge_file};

const CLAUDE_AGENT: &str = "---\r\nname: code-reviewer\r\ndescription: >\r\n  Reviews code for bugs\r\n  and style.\r\ntools: Read, Grep, Glob, Bash(git:*), Task\r\nmodel: sonnet\r\ncolor: blue\r\n---\r\nYou review code.\r\n";

#[test]
fn parses_claude_agent_file() {
    let fm = Frontmatter::parse(CLAUDE_AGENT);
    assert_eq!(fm.text("name").as_deref(), Some("code-reviewer"));
    assert_eq!(
        fm.text("description").as_deref(),
        Some("Reviews code for bugs and style.")
    );
    assert_eq!(fm.body, "You review code.");
    let mut agents = Vec::new();
    merge_file(
        &mut agents,
        Path::new("/g/reviewer.md"),
        CLAUDE_AGENT,
        Source::Global,
    );
    let agent = &agents[0];
    assert_eq!(agent.name, "code-reviewer");
    assert_eq!(
        agent.tools.as_deref(),
        Some(&["read", "grep", "find", "bash"].map(String::from)[..])
    );
    assert_eq!(
        agent.model, None,
        "Claude aliases inherit the session model"
    );
    assert_eq!(agent.prompt, "You review code.");
    assert!(!agent.builtin);
}

#[test]
fn parses_lists_quotes_and_pi_keys() {
    let text = "---\nname: \"x: y\"\ntools:\n  - read\n  - 'ls'\nthinking: high\nmax_turns: 7\nsystemPromptMode: replace\nmodel: openai/gpt-5 # pinned\n---\nbody";
    let fm = Frontmatter::parse(text);
    assert_eq!(fm.text("name").as_deref(), Some("x: y"));
    assert_eq!(
        fm.get("tools"),
        Some(&FmValue::List(vec!["read".into(), "ls".into()]))
    );
    let mut agents = Vec::new();
    merge_file(&mut agents, Path::new("/g/a.md"), text, Source::Global);
    let agent = &agents[0];
    assert_eq!(agent.name, "x--y");
    assert_eq!(agent.thinking.as_deref(), Some("high"));
    assert_eq!(agent.max_turns, Some(7));
    assert_eq!(agent.prompt_mode, PromptMode::Replace);
    assert_eq!(agent.model.as_deref(), Some("openai/gpt-5"));
}

#[test]
fn render_round_trips() {
    let mut fm = Frontmatter::parse("---\nname: a\n---\nhello");
    fm.set("description", Some(FmValue::Text("Use it: often".into())));
    fm.set(
        "tools",
        Some(FmValue::List(vec!["read".into(), "bash".into()])),
    );
    let again = Frontmatter::parse(&fm.render());
    assert_eq!(again.text("description").as_deref(), Some("Use it: often"));
    assert_eq!(again.get("tools").unwrap().as_list(), ["read", "bash"]);
    assert_eq!(again.body, "hello");
    fm.set("tools", None);
    assert!(Frontmatter::parse(&fm.render()).get("tools").is_none());
}

#[test]
fn maps_claude_tool_names() {
    let names = [
        "Read",
        "Write",
        "Edit",
        "MultiEdit",
        "Glob",
        "LS",
        "TodoWrite",
        "WebFetch",
        "mcp__gh__x",
    ];
    let mapped = map_tools(&names.map(String::from));
    assert_eq!(
        mapped,
        [
            "read",
            "write",
            "edit",
            "find",
            "ls",
            "todo",
            "webfetch",
            "mcp__gh__x"
        ]
    );
}

fn write(dir: &Path, file: &str, text: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file), text).unwrap();
}

#[test]
fn layers_override_per_field() {
    let root = pi_ext_e2e::TempDir::new().unwrap();
    let agent_dir = root.path().join("agent");
    let project = root.path().join("work/.pi");
    let global = registry::agents_dir(&agent_dir);
    let local = registry::agents_dir(&project);
    write(
        &global,
        "explore.md",
        "---\nname: explore\nmodel: anthropic/claude-x\nthinking: low\n---\n",
    );
    write(
        &local,
        "explore.md",
        "---\nthinking: max\ntools: read, grep\n---\nProject prompt",
    );
    write(
        &local,
        "docs.md",
        "---\ndescription: Writes docs\n---\nYou write docs.",
    );
    let agents = registry::load(&Layers::new(&agent_dir, Some(&project)));
    let explore = agents.iter().find(|a| a.name == "explore").unwrap();
    assert!(explore.builtin && explore.overridden());
    assert_eq!(explore.source, Source::Project);
    assert_eq!(explore.model.as_deref(), Some("anthropic/claude-x"));
    assert_eq!(explore.thinking.as_deref(), Some("max"));
    assert_eq!(
        explore.tools.as_deref(),
        Some(&["read".to_string(), "grep".to_string()][..])
    );
    assert_eq!(explore.prompt, "Project prompt");
    assert!(
        explore.description.contains("Read-only"),
        "description stays built-in"
    );
    assert_eq!(explore.files.len(), 2);
    let docs = agents.iter().find(|a| a.name == "docs").unwrap();
    assert_eq!(docs.description, "Writes docs");
    let build = agents.iter().find(|a| a.name == "build").unwrap();
    assert_eq!(build.tools, None);
    assert!(!build.overridden());

    let untrusted = registry::load(&Layers::new(&agent_dir, None));
    assert!(untrusted.iter().all(|a| a.name != "docs"));
    let explore = untrusted.iter().find(|a| a.name == "explore").unwrap();
    assert_eq!(explore.thinking.as_deref(), Some("low"));
}

#[test]
fn edit_file_updates_existing_definition() {
    let root = pi_ext_e2e::TempDir::new().unwrap();
    let dir = root.path().join("agents");
    write(
        &dir,
        "my-file.md",
        "---\nname: advisor\nthinking: low\n---\nOld",
    );
    let path = registry::edit_file(&dir, "advisor", |fm| {
        fm.set("model", Some(FmValue::Text("openai/o9".into())));
        fm.body = "New".into();
    })
    .unwrap();
    assert!(path.ends_with("my-file.md"));
    let fm = Frontmatter::parse(&std::fs::read_to_string(&path).unwrap());
    assert_eq!(fm.text("thinking").as_deref(), Some("low"));
    assert_eq!(fm.text("model").as_deref(), Some("openai/o9"));
    assert_eq!(fm.body, "New");
    let fresh = registry::edit_file(&dir, "build", |fm| fm.body = "B".into()).unwrap();
    assert!(fresh.ends_with("build.md"));
}

#[test]
fn reads_disallowed_tools() {
    let text = "---\nname: rev\ndisallowedTools: Bash, Write\n---\nReview.\n";
    let mut agents = Vec::new();
    merge_file(&mut agents, Path::new("/g/rev.md"), text, Source::Global);
    assert_eq!(agents[0].disallowed_tools, ["bash", "write"]);
    assert_eq!(agents[0].tools, None);
}
