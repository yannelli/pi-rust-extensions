//! The hooks that Codex's bundled plugins send to MCP servers and apps inside Codex, from the
//! allowlist in codex-rs/plugin/src/bundled_hooks.rs (Codex 0.171.0).

use crate::model::HookDef;

const CLEANUP_EVENTS: [&str; 3] = ["Stop", "Interrupt", "SubagentStop"];

/// `(plugin id, server, tool)` of each cleanup hook. `codex_apps` targets need an empty input.
const CLEANUP_HOOKS: [(&str, &str, &str); 7] = [
    ("browser@openai-bundled", "node_repl", "turn_ended"),
    ("chrome@openai-bundled", "node_repl", "turn_ended"),
    ("chrome-dev@openai-bundled", "node_repl", "turn_ended"),
    ("chrome-internal@openai-bundled", "node_repl", "turn_ended"),
    ("computer-use@openai-bundled", "node_repl", "turn_ended"),
    (
        "unified-computer-use@openai-bundled",
        "cua_repl",
        "turn_ended",
    ),
    (
        "browser@openai-curated-remote",
        "codex_apps",
        "browser.turn_ended",
    ),
];

/// True for a hook on Codex's bundled-hook allowlist: a cleanup hook on `CLEANUP_EVENTS`
/// without a matcher, or the SessionStart hook of `codex-dream-memory@openai-bundled`.
pub fn is_codex_bundled(plugin_id: &str, def: &HookDef) -> bool {
    if def.kind() != "mcp_tool" {
        return false;
    }
    let server = def.str_field("server").unwrap_or_default();
    let tool = def.str_field("tool").unwrap_or_default();
    let no_input = def
        .handler
        .get("input")
        .is_none_or(|input| input.as_object().is_some_and(|map| map.is_empty()));
    let cleanup = def.matcher.is_none()
        && CLEANUP_EVENTS.contains(&def.event.as_str())
        && CLEANUP_HOOKS.iter().any(|&(id, hook_server, hook_tool)| {
            id == plugin_id
                && hook_server == server
                && hook_tool == tool
                && (server != "codex_apps" || no_input)
        });
    let dream_memory = plugin_id == "codex-dream-memory@openai-bundled"
        && def.event == "SessionStart"
        && def.matcher.as_deref() == Some("^(startup|compact)$")
        && server == "codex_apps"
        && tool == "codex_dream_memory.get_mec"
        && no_input;
    cleanup || dream_memory
}
