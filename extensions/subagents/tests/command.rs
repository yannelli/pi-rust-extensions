use std::path::Path;

use pi_ext_subagents::agent::PromptMode;
use pi_ext_subagents::command::{self, ChildInput, Inherit, Overrides};
use pi_ext_subagents::defaults::builtin_agents;
use pi_ext_subagents::stream::{Effect, Progress};
use serde_json::json;

fn parent() -> Inherit {
    Inherit {
        model: Some("mock/parent".into()),
        thinking: Some("high".into()),
    }
}

fn args_of(
    agent: &pi_ext_subagents::agent::AgentDef,
    overrides: &Overrides,
    task: &str,
) -> command::ChildSpec {
    command::build(ChildInput {
        agent,
        task,
        run_id: "run1",
        parent: &parent(),
        overrides,
        program: Path::new("/bin/pi"),
        cwd: Path::new("/work"),
        extra_args: &["--offline".into()],
    })
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

#[test]
fn explore_inherits_parent_model_and_thinking() {
    let explore = builtin_agents().remove(0);
    let spec = args_of(&explore, &Overrides::default(), "find main");
    assert_eq!(&spec.args[..4], ["--mode", "json", "-p", "--no-session"]);
    assert_eq!(flag(&spec.args, "--model"), Some("mock/parent"));
    assert_eq!(flag(&spec.args, "--thinking"), Some("high"));
    assert_eq!(flag(&spec.args, "--tools"), Some("read,grep,find,ls,todo"));
    assert!(
        flag(&spec.args, "--append-system-prompt")
            .unwrap()
            .contains("explore subagent")
    );
    assert!(spec.args.contains(&"--offline".to_string()));
    assert_eq!(&spec.args[spec.args.len() - 2..], ["--", "find main"]);
    assert!(spec.env.contains(&("PI_TODO_SCOPE".into(), "run1".into())));
    assert_eq!(spec.cwd, Path::new("/work"));
}

#[test]
fn agent_model_drops_parent_thinking() {
    let mut build = builtin_agents().remove(1);
    build.model = Some("openai/gpt-x".into());
    build.max_turns = Some(5);
    build.prompt_mode = PromptMode::Replace;
    let spec = args_of(&build, &Overrides::default(), "@file task");
    assert_eq!(flag(&spec.args, "--model"), Some("openai/gpt-x"));
    assert_eq!(flag(&spec.args, "--thinking"), None);
    assert_eq!(flag(&spec.args, "--exclude-tools"), Some("subagent"));
    assert_eq!(flag(&spec.args, "--max-turns"), Some("5"));
    assert!(flag(&spec.args, "--system-prompt").is_some());
    assert_eq!(spec.args.last().map(String::as_str), Some(" @file task"));

    build.thinking = Some("low".into());
    let spec = args_of(&build, &Overrides::default(), "t");
    assert_eq!(flag(&spec.args, "--thinking"), Some("low"));
}

#[test]
fn call_overrides_win() {
    let advisor = builtin_agents().remove(2);
    let overrides = Overrides {
        model: Some("x/y".into()),
        thinking: Some("max".into()),
    };
    let spec = args_of(&advisor, &overrides, "review");
    assert_eq!(flag(&spec.args, "--model"), Some("x/y"));
    assert_eq!(flag(&spec.args, "--thinking"), Some("max"));
    let thinking_only = Overrides {
        model: None,
        thinking: Some("off".into()),
    };
    let spec = args_of(&advisor, &thinking_only, "review");
    assert_eq!(flag(&spec.args, "--model"), Some("mock/parent"));
    assert_eq!(flag(&spec.args, "--thinking"), Some("off"));
}

#[test]
fn no_parent_model_passes_no_model_flag() {
    let explore = builtin_agents().remove(0);
    let spec = command::build(ChildInput {
        agent: &explore,
        task: "t",
        run_id: "r",
        parent: &Inherit::default(),
        overrides: &Overrides::default(),
        program: Path::new("pi"),
        cwd: Path::new("/"),
        extra_args: &[],
    });
    assert_eq!(flag(&spec.args, "--model"), None);
    assert_eq!(flag(&spec.args, "--thinking"), None);
}

#[test]
fn progress_folds_event_stream() {
    let mut progress = Progress::default();
    let lines = [
        json!({"type": "turn_start"}),
        json!({"type": "tool_execution_start", "toolCallId": "a", "toolName": "bash", "args": {"command": "ls -la\nmore"}}),
        json!({"type": "message_start", "message": {"role": "assistant", "content": []}}),
        json!({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": "Hi"}}),
    ];
    for line in &lines {
        progress.apply_line(&line.to_string());
    }
    assert_eq!(progress.turns, 1);
    assert_eq!(progress.tools[0].summary, "ls -la");
    assert!(!progress.tools[0].done);
    assert_eq!(progress.text, "Hi");
    let todo = json!({"type": "tool_execution_end", "toolCallId": "b", "toolName": "todo", "isError": false,
        "result": {"details": {"tasks": []}}});
    assert_eq!(progress.apply(&todo), Effect::Todo(json!({"tasks": []})));
    let end = json!({"type": "message_end", "message": {"role": "assistant", "stopReason": "error",
        "errorMessage": "rate limited", "content": [{"type": "text", "text": "partial"}]}});
    progress.apply(&end);
    assert_eq!(progress.error.as_deref(), Some("rate limited"));
    assert_eq!(progress.output(), "partial");
    assert_eq!(progress.apply_line("not json"), Effect::None);
}
