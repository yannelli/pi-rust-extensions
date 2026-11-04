use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use pi_ext_sdk::prelude::*;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config::Config;
use crate::pipeline::{Runner, Scan};
use crate::{detection, diagnostics, state::State};

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
        crate::events::refresh_if_changed(ctx.host(), &state).await;
        let (config, known, runner) = {
            let state = state.lock().await;
            let known: Vec<_> = state.findings.keys().cloned().collect();
            (state.config.clone(), known, state.runner.clone())
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
        let scan = scan_lsp(&config, &cwd, scan_paths, &runner, &state);
        tokio::select! {
            () = scan => {}
            () = ctx.cancelled() => return Err(Error::cancelled()),
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

/// Requests LSP diagnostics for each path within one `pipelineTimeoutMs` budget.
/// Errors and skipped servers become warning findings.
async fn scan_lsp(
    config: &Config,
    cwd: &Path,
    paths: Vec<std::path::PathBuf>,
    runner: &Runner,
    state: &Arc<Mutex<State>>,
) {
    let deadline = Instant::now() + Duration::from_millis(config.pipeline_timeout_ms);
    for path in paths {
        let Some(language) = config.file_language(&path) else {
            continue;
        };
        if !config.language_enabled(&language) || !diagnostics::within(cwd, &path) {
            continue;
        }
        let root = detection::root(&path, cwd, &language);
        let scan = Scan {
            config,
            cwd,
            root: &root,
            path: &path,
            language: &language,
            deadline,
        };
        let findings = scan.servers(runner, false).await;
        let mut state = state.lock().await;
        let current = state.findings.entry(path).or_default();
        current.retain(|item| item["kind"] != "lsp");
        current.extend(findings);
    }
}
