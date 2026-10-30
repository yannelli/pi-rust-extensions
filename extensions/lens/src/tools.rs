use std::path::Path;
use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;

use crate::{detection, diagnostics, lsp, state::State};

pub(crate) fn bounded(mut text: String) -> String {
    if text.len() > 40 * 1024 {
        let mut end = 40 * 1024 - 30;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[Lens output truncated]");
    }
    text
}

pub(crate) async fn get_diagnostics(
    args: Value,
    ctx: ToolCtx,
    state: Arc<Mutex<State>>,
) -> Result<ToolResult> {
    let cwd = ctx.host().cwd();
    let paths: Vec<_> = args["paths"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|path| diagnostics::normalize_path(&cwd, path))
        .collect();
    if args["source"] == "lsp" {
        let (config, known) = {
            let state = state.lock().await;
            let known: Vec<_> = state.findings.keys().cloned().collect();
            (state.config.clone(), known)
        };
        if !config.enabled {
            return Err(Error::msg("Lens is disabled"));
        }
        if config.lsp.servers.iter().all(|server| !server.enabled) {
            return Err(Error::msg("No Lens LSP server is configured"));
        }
        let scan_paths = if paths.is_empty() {
            known
        } else {
            paths.clone()
        };
        for path in scan_paths {
            let Some(language) = detection::language(&path) else {
                continue;
            };
            if !config.language_enabled(language) || !diagnostics::within(&cwd, &path) {
                continue;
            }
            let root = detection::root(&path, &cwd, language);
            let mut findings = vec![];
            for server in &config.lsp.servers {
                if server.enabled && server.languages.iter().any(|item| item == language) {
                    findings.extend(
                        lsp::diagnostics(server, &root, &path, config.timeout_ms)
                            .await
                            .map_err(Error::msg)?
                            .into_iter()
                            .map(|mut item| {
                                item["kind"] = json!("lsp");
                                item
                            }),
                    );
                }
            }
            let mut state = state.lock().await;
            let current = state.findings.entry(path).or_default();
            current.retain(|item| item["kind"] != "lsp");
            current.extend(findings);
        }
    }
    let state = state.lock().await;
    let findings: Vec<_> = state
        .all_findings()
        .into_iter()
        .filter(|item| {
            let path_matches = args["scope"] == "workspace"
                || paths.is_empty()
                || paths
                    .iter()
                    .any(|path| Some(path.as_path()) == item["path"].as_str().map(Path::new));
            path_matches && (args["severity"].is_null() || args["severity"] == item["severity"])
        })
        .collect();
    let mut kept = vec![];
    let mut bytes = 0;
    let count = findings.len();
    for finding in findings {
        let size = serde_json::to_vec(&finding)?.len();
        if bytes + size > 18 * 1024 {
            break;
        }
        bytes += size;
        kept.push(finding);
    }
    let truncated = kept.len() != count;
    let text = serde_json::to_string(&kept)?;
    Ok(
        ToolResult::text(text)
            .details(json!({"findings":kept,"truncated":truncated,"total":count})),
    )
}
