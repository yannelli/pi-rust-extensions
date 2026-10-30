use crate::agent::{AgentDef, PromptMode, Source};

pub const BUILTIN_NAMES: [&str; 3] = ["explore", "build", "advisor"];

const READ_ONLY: [&str; 5] = ["read", "grep", "find", "ls", "todo"];

const EXPLORE: &str =
    "You are the explore subagent. You search and read the codebase to answer one \
question for the parent agent. You do not change files.

- Start broad (find, grep), then read only the parts that answer the question.
- Report what you found with file paths and line numbers, and quote short code excerpts when they \
carry the answer.
- Say what you did not find or could not confirm.
- End with a short, direct answer to the question.";

const BUILD: &str = "You are the build subagent. You implement one task for the parent agent with \
the full tool set.

- Read the code you change and follow its style.
- Make the smallest change that completes the task, then run the checks that cover it.
- Do not stop halfway. When you are blocked, say what blocks you.
- End with a summary: files changed (with paths), checks run and their results, open issues.";

const ADVISOR: &str =
    "You are the advisor subagent. You review a plan, a change, or a question for \
the parent agent and give a recommendation. You do not change files.

- Read the code that the question touches before you judge it.
- Name concrete problems: wrong assumptions, missed cases, simpler alternatives, risks.
- Rank the points by importance and keep each one short.
- End with one clear recommendation and the reason for it.";

pub fn builtin_agents() -> Vec<AgentDef> {
    let read_only = || Some(READ_ONLY.iter().map(|t| (*t).to_owned()).collect());
    vec![
        builtin(
            "explore",
            "Read-only codebase search: finds files, symbols, and facts and reports them with paths.",
            EXPLORE,
            read_only(),
        ),
        builtin(
            "build",
            "Implements a task with the full tool set and reports the files changed and checks run.",
            BUILD,
            None,
        ),
        builtin(
            "advisor",
            "Read-only reviewer: checks a plan or change and answers with a ranked review and a recommendation.",
            ADVISOR,
            read_only(),
        ),
    ]
}

fn builtin(name: &str, description: &str, prompt: &str, tools: Option<Vec<String>>) -> AgentDef {
    AgentDef {
        name: name.to_owned(),
        description: description.to_owned(),
        prompt: prompt.to_owned(),
        tools,
        model: None,
        thinking: None,
        max_turns: None,
        prompt_mode: PromptMode::Append,
        source: Source::Builtin,
        builtin: true,
        files: Vec::new(),
    }
}
