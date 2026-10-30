//! `/statusline [choice]`: the editor panel, or a direct switch.

use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::config::status_line_choice;
use crate::editor::StatusLineEditor;
use crate::external::validate_source;
use crate::footer::{DEFAULT_WIDTH, Footer};
use crate::sources;

pub async fn statusline_command(footer: Arc<Footer>, args: String, ctx: CommandCtx) -> Result<()> {
    let host = ctx.host().clone();
    let choice = args.trim();
    if choice.is_empty() || choice == "edit" {
        return open_editor(footer, host).await;
    }
    if let Err(error) = validate_source(choice, footer.home()) {
        return host.notify_error(error).await;
    }
    match status_line_choice(Footer::global_value(&host), choice) {
        Ok(value) => {
            if let Err(error) = sources::write_global(&host.agent_dir(), &value) {
                return host.notify_error(error).await;
            }
            footer.reload(&host).await;
            host.notify_info(format!("Status line: {choice}")).await
        }
        Err(message) => host.notify_error(message).await,
    }
}

async fn open_editor(footer: Arc<Footer>, host: Host) -> Result<()> {
    let options = if Footer::trusted(&host) {
        vec!["Global", "Project"]
    } else {
        vec!["Global"]
    };
    let Some(scope) = host.select("Status line scope", &options).await? else {
        return Ok(());
    };
    let (path, value) = match scope.as_str() {
        "Global" => (
            sources::global_config_path(&host.agent_dir()),
            Footer::global_value(&host),
        ),
        "Project" if Footer::trusted(&host) => {
            let path = sources::project_config_path(&host.cwd());
            let value = sources::read_object(&path).map(Value::Object);
            (path, value)
        }
        _ => return Ok(()),
    };
    let save_path = path.clone();
    let save_host = host.clone();
    let save = Box::new(move |value: Value| {
        if scope == "Project" && !Footer::trusted(&save_host) {
            return Err("Project is not trusted.".into());
        }
        sources::write_config(&save_path, &value)
    });
    let mut editor =
        StatusLineEditor::new(value, footer.home().to_path_buf(), save).with_config_path(path);
    let render = |editor: &StatusLineEditor, width: usize| {
        editor.render(width, &footer.preview(&editor.value, width))
    };
    let mut panel = host.panel(render(&editor, DEFAULT_WIDTH)).open().await?;
    loop {
        match panel.next_event().await {
            PanelEvent::Key(key) => {
                let before = editor.value.clone();
                editor.handle_key(&key);
                if editor.value != before {
                    footer.reload(&host).await;
                }
                if editor.closed {
                    break;
                }
            }
            PanelEvent::Resize(_) => {}
            PanelEvent::Closed => return Ok(()),
        }
        let width = panel.width().unwrap_or(DEFAULT_WIDTH);
        panel.set_lines(render(&editor, width))?;
    }
    panel.close(Value::Null).await.map(|_| ())
}
