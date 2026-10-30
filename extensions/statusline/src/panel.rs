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
    let agent_dir = host.agent_dir();
    let save = Box::new(move |value: Value| sources::write_global(&agent_dir, &value));
    let mut editor = StatusLineEditor::new(
        Footer::global_value(&host),
        footer.home().to_path_buf(),
        save,
    );
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
