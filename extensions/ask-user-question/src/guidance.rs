//! Tool description, prompt snippet, guidelines, and host error texts from upstream.

use pi_ext_sdk::ToolSpec;

use crate::config::Guidance;
use crate::schema::{MAX_OPTIONS, MAX_QUESTIONS, MIN_OPTIONS, parameters_schema};

pub const TOOL_NAME: &str = "ask_user_question";
pub const TOOL_LABEL: &str = "Ask User Question";

pub const ERROR_NO_UI: &str = "Error: UI not available (running in non-interactive mode)";
pub const ERROR_NO_CUSTOM_UI: &str = "Error: this client cannot render the questionnaire (custom UI is unavailable, e.g. RPC/ACP hosts such as Zed or Paseo). The user never saw the questions — do NOT treat this as a decline. Ask the questions as plain chat text instead, without using this tool.";

pub const DEFAULT_TOOL_DESCRIPTION: &str = r#"Ask the user one or more structured questions during execution. Use when you need to:
1. Gather user preferences or requirements
2. Clarify ambiguous instructions
3. Get decisions on implementation choices as you work
4. Offer choices to the user about what direction to take

Usage notes:
- Users can type a custom answer via the automatically appended "Type something." row on every question or press Esc to abandon the questionnaire. Do NOT author "Other" or "Type something." labels yourself — reserved labels are rejected at runtime.
- Use multiSelect: true when multiple answers are valid. The "Type something." row is available on every question, including when options carry a `preview`; in preview mode it expands to the full pane width while typing so the custom answer is not cramped into the narrow options column.
- If you recommend a specific option, make that the first option in the list and add "(Recommended)" at the end of the label.

Preview feature:
Use the optional `preview` field on options when presenting concrete artifacts that users need to visually compare:
- ASCII mockups of UI layouts or components
- Code snippets showing different implementations
- Diagram variations
- Configuration examples

Preview content is rendered as markdown in a monospace box. Multi-line text with newlines is supported. When any option has a preview, the UI switches to a side-by-side layout with a vertical option list on the left and preview on the right. Do not use previews for simple preference questions where labels and descriptions suffice. Note: previews are only supported for single-select questions (not multiSelect)."#;

pub fn default_prompt_snippet() -> String {
    format!(
        "Ask the user up to {MAX_QUESTIONS} structured questions ({MIN_OPTIONS}-{MAX_OPTIONS} options each) when requirements are ambiguous"
    )
}

pub fn default_prompt_guidelines() -> Vec<String> {
    vec![
        format!(
            "Use ask_user_question whenever the user's request is underspecified and you cannot proceed without concrete decisions — you can ask up to {MAX_QUESTIONS} questions per invocation."
        ),
        format!(
            "Each question MUST have {MIN_OPTIONS}-{MAX_OPTIONS} options. Every option requires a concise label (1-5 words) and a description explaining what the choice means or its trade-offs. The user can additionally type a custom answer via the automatically appended \"Type something.\" row on every question, or press Esc to abandon the questionnaire. Do NOT author \"Other\" or \"Type something.\" labels yourself — reserved labels are rejected at runtime."
        ),
        "Set multiSelect: true when multiple answers are valid. Provide an options[].preview markdown string when an option benefits from richer side-by-side context (mockups, code snippets, diagrams, configs) — single-select only. The \"Type something.\" row is appended to every question; in preview mode it expands to the full pane width while typing so the custom answer is not cramped into the narrow options column. If you recommend a specific option, make that the first option and append \"(Recommended)\" to its label.".to_owned(),
        "Do not stack multiple ask_user_question calls back-to-back — group all clarifying questions into one invocation.".to_owned(),
    ]
}

pub fn tool_spec(guidance: &Guidance) -> ToolSpec {
    let description = guidance
        .description
        .clone()
        .unwrap_or_else(|| DEFAULT_TOOL_DESCRIPTION.to_owned());
    let snippet = guidance
        .prompt_snippet
        .clone()
        .unwrap_or_else(default_prompt_snippet);
    let guidelines = guidance
        .prompt_guidelines
        .clone()
        .unwrap_or_else(default_prompt_guidelines);
    guidelines.into_iter().fold(
        ToolSpec::new(TOOL_NAME, description)
            .label(TOOL_LABEL)
            .parameters(parameters_schema())
            .prompt_snippet(snippet),
        ToolSpec::guideline,
    )
}
