//! `/ollama`: the panel and the subcommands.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;
use pi_ext_sdk::tokio::sync::mpsc;

use crate::client::{PullProgress, normalize_host};
use crate::state::{Ollama, write_global_host};
use crate::view::{Action, Mode, View, progress_text};

const WIDTH: usize = 100;

fn refreshed(result: std::result::Result<usize, String>) -> String {
    match result {
        Ok(count) => format!("Registered {count} Ollama models as provider ollama."),
        Err(error) => format!("Ollama refresh failed: {error}"),
    }
}

pub async fn run(ollama: Arc<Ollama>, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    let mut words = args.split_whitespace();
    let sub = words.next().unwrap_or("");
    let rest: Vec<&str> = words.collect();
    let rest = rest.join(" ");
    match sub {
        "" | "list" => open_panel(ollama, host).await,
        "refresh" => {
            let result = ollama.refresh(&host).await;
            match result {
                Ok(_) => host.notify_info(refreshed(result)).await,
                Err(_) => host.notify_error(refreshed(result)).await,
            }
        }
        "host" if rest.is_empty() => host.notify_info(format!("Ollama host: {}", ollama.host(&host).await)).await,
        "host" => set_host(&ollama, &host, &rest).await,
        "pull" if !rest.is_empty() => pull_with_status(&ollama, &host, &rest).await,
        "delete" if !rest.is_empty() => {
            if !host.confirm("Delete Ollama model?", &format!("Delete {rest} from Ollama?")).await? {
                return Ok(());
            }
            match ollama.client(&host).await.delete(&rest).await {
                Ok(()) => host.notify_info(format!("Deleted {rest}. {}", refreshed(ollama.refresh(&host).await))).await,
                Err(error) => host.notify_error(format!("Delete failed: {error}")).await,
            }
        }
        "show" if !rest.is_empty() => {
            let _ = ollama.refresh(&host).await;
            match ollama.models().into_iter().find(|m| m.tag.name == rest) {
                Some(model) => host.notify_info(model.details().join("\n")).await,
                None => host.notify_error(format!("No Ollama model named {rest}.")).await,
            }
        }
        _ => host.notify_error("Usage: /ollama [list | refresh | pull <model> | delete <model> | show <model> | host [url]]").await,
    }
}

async fn set_host(ollama: &Ollama, host: &Host, url: &str) -> Result<()> {
    let url = normalize_host(url);
    if let Err(error) = write_global_host(&host.agent_dir(), &url) {
        return host.notify_error(error).await;
    }
    host.notify_info(format!(
        "Ollama host set to {url}. {}",
        refreshed(ollama.refresh(host).await)
    ))
    .await
}

async fn pull_with_status(ollama: &Ollama, host: &Host, model: &str) -> Result<()> {
    let client = ollama.client(host).await;
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let name = model.to_string();
    let pull = tokio::spawn(async move {
        client
            .pull(&name, |update: &PullProgress| {
                let _ = tx.send(progress_text(update));
            })
            .await
    });
    while let Some(text) = rx.recv().await {
        let _ = host
            .set_status("ollama", Some(&format!("ollama: {model}: {text}")))
            .await;
    }
    let _ = host.set_status("ollama", None).await;
    match pull.await.map_err(|e| Error::msg(e.to_string()))? {
        Ok(()) => {
            host.notify_info(format!(
                "Pulled {model}. {}",
                refreshed(ollama.refresh(host).await)
            ))
            .await
        }
        Err(error) => {
            host.notify_error(format!("Pull of {model} failed: {error}"))
                .await
        }
    }
}

async fn open_panel(ollama: Arc<Ollama>, host: Host) -> Result<()> {
    let result = ollama.refresh(&host).await;
    let mut view = View::new(ollama.host(&host).await, ollama.models());
    if let Err(error) = result {
        view.message = Some(format!("Ollama is not reachable: {error}"));
    }
    let mut panel = host.panel(view.render(WIDTH)).open().await?;
    let (tx, mut rx) = mpsc::unbounded_channel::<std::result::Result<Option<String>, String>>();
    let mut pull: Option<tokio::task::JoinHandle<()>> = None;
    loop {
        let action = tokio::select! {
            event = panel.next_event() => match event {
                PanelEvent::Key(key) => view.handle_key(&key),
                PanelEvent::Resize(_) => Action::None,
                PanelEvent::Closed => {
                    if let Some(task) = pull.take() {
                        task.abort();
                    }
                    return Ok(());
                }
            },
            Some(update) = rx.recv() => {
                match update {
                    Ok(Some(text)) => view.progress = Some(text),
                    Ok(None) => {
                        let name = match &view.mode { Mode::Pulling(name) => name.clone(), _ => String::new() };
                        view.mode = Mode::List;
                        view.message = Some(format!("Pulled {name}. {}", refreshed(ollama.refresh(&host).await)));
                        view.set_models(ollama.models());
                    }
                    Err(error) => {
                        view.mode = Mode::List;
                        view.message = Some(format!("Pull failed: {error}"));
                    }
                }
                Action::None
            }
        };
        match action {
            Action::None => {}
            Action::Close => break,
            Action::Refresh => {
                view.message = Some(refreshed(ollama.refresh(&host).await));
                view.set_models(ollama.models());
            }
            Action::Delete(name) => {
                let result = ollama.client(&host).await.delete(&name).await;
                view.message = Some(match result {
                    Ok(()) => format!("Deleted {name}. {}", refreshed(ollama.refresh(&host).await)),
                    Err(error) => format!("Delete failed: {error}"),
                });
                view.set_models(ollama.models());
            }
            Action::SetHost(url) => {
                let url = normalize_host(&url);
                view.message = Some(match write_global_host(&host.agent_dir(), &url) {
                    Ok(()) => refreshed(ollama.refresh(&host).await),
                    Err(error) => error,
                });
                view.host = ollama.host(&host).await;
                view.set_models(ollama.models());
            }
            Action::Pull(name) => {
                view.mode = Mode::Pulling(name.clone());
                view.progress = None;
                let (client, tx) = (ollama.client(&host).await, tx.clone());
                pull = Some(tokio::spawn(async move {
                    let progress = tx.clone();
                    let result = client.pull(&name, |update| {
                        let _ = progress.send(Ok(Some(progress_text(update))));
                    });
                    let _ = tx.send(result.await.map(|()| None));
                }));
            }
            Action::CancelPull => {
                if let Some(task) = pull.take() {
                    task.abort();
                }
                view.mode = Mode::List;
                view.message = Some("Pull canceled.".into());
            }
        }
        let width = panel.width().unwrap_or(WIDTH);
        panel.set_lines(view.render(width))?;
    }
    if let Some(task) = pull {
        task.abort();
    }
    panel.close(Value::Null).await.map(|_| ())
}
