//! Tool guidance and host error texts.

use pi_ext_sdk::ToolSpec;

use crate::config::Guidance;
use crate::schema::parameters_schema;

pub const TOOL_NAME: &str = "ask_user_question";
pub const TOOL_LABEL: &str = "Ask User Question";

pub const ERROR_NO_UI: &str = "Error: UI not available (running in non-interactive mode)";
pub const ERROR_NO_CUSTOM_UI: &str = "Error: this client cannot render the questionnaire (custom UI is unavailable, e.g. RPC/ACP hosts such as Zed or Paseo). The user never saw the questions — do NOT treat this as a decline. Ask the questions as plain chat text instead, without using this tool.";

pub const DEFAULT_TOOL_DESCRIPTION: &str =
    "Ask structured questions when user decisions are needed to proceed.";

pub fn default_prompt_snippet() -> String {
    "Ask for user decisions".into()
}

pub fn default_prompt_guidelines() -> Vec<String> {
    vec![
        "Group clarifying questions in one ask_user_question call. Put a recommended option first with \"(Recommended)\" appended to its label. The UI adds custom input; Esc cancels.".into(),
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
