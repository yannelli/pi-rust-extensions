use serde::Serialize;
use serde_json::{Value, json};

use super::{Host, lines, without_nulls};
use crate::error::Result;
use crate::panel::PanelBuilder;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyType {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Placement {
    #[default]
    AboveEditor,
    BelowEditor,
}

/// `ui.*` requests. Dialogs fail with `Error::is_no_ui` when pi has no UI.
/// Dropping a dialog future (for example with `tokio::time::timeout`) closes the dialog.
impl Host {
    pub async fn notify_as(&self, message: impl Into<String>, kind: NotifyType) -> Result<()> {
        let params = json!({ "message": message.into(), "type": kind });
        self.call("ui.notify", params).await.map(drop)
    }

    pub async fn notify_info(&self, message: impl Into<String>) -> Result<()> {
        self.notify_as(message, NotifyType::Info).await
    }

    pub async fn notify_warning(&self, message: impl Into<String>) -> Result<()> {
        self.notify_as(message, NotifyType::Warning).await
    }

    pub async fn notify_error(&self, message: impl Into<String>) -> Result<()> {
        self.notify_as(message, NotifyType::Error).await
    }

    /// The chosen option, or `None` when the user dismissed the dialog.
    pub async fn select<S: AsRef<str>>(
        &self,
        title: &str,
        options: &[S],
    ) -> Result<Option<String>> {
        let options: Vec<&str> = options.iter().map(AsRef::as_ref).collect();
        let params = json!({ "title": title, "options": options });
        self.call_as("ui.select", params).await
    }

    pub async fn confirm(&self, title: &str, message: &str) -> Result<bool> {
        let params = json!({ "title": title, "message": message });
        let answer: Option<bool> = self.call_as("ui.confirm", params).await?;
        Ok(answer.unwrap_or(false))
    }

    pub async fn input(&self, title: &str, placeholder: Option<&str>) -> Result<Option<String>> {
        let params = without_nulls(json!({ "title": title, "placeholder": placeholder }));
        self.call_as("ui.input", params).await
    }

    pub async fn editor(&self, title: &str, prefill: Option<&str>) -> Result<Option<String>> {
        let params = without_nulls(json!({ "title": title, "prefill": prefill }));
        self.call_as("ui.editor", params).await
    }

    /// Sets (`Some`) or clears (`None`) a status entry.
    pub async fn set_status(&self, key: &str, text: Option<&str>) -> Result<()> {
        let params = json!({ "key": key, "text": text });
        self.call("ui.setStatus", params).await.map(drop)
    }

    /// Shows lines in widget `key` (surface `widget:<key>`).
    pub async fn set_widget<I, S>(&self, key: &str, lines_: I, placement: Placement) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let params = json!({ "key": key, "lines": lines(lines_), "placement": placement });
        self.call("ui.setWidget", params).await.map(drop)
    }

    pub async fn clear_widget(&self, key: &str) -> Result<()> {
        let params = json!({ "key": key, "lines": Value::Null });
        self.call("ui.setWidget", params).await.map(drop)
    }

    /// Replaces the footer with lines (surface `footer`).
    pub async fn set_footer<I, S>(&self, lines_: I) -> Result<()>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let params = json!({ "lines": lines(lines_) });
        self.call("ui.setFooter", params).await.map(drop)
    }

    /// Restores the default footer.
    pub async fn clear_footer(&self) -> Result<()> {
        let params = json!({ "lines": Value::Null });
        self.call("ui.setFooter", params).await.map(drop)
    }

    pub async fn set_title(&self, title: &str) -> Result<()> {
        let params = json!({ "title": title });
        self.call("ui.setTitle", params).await.map(drop)
    }

    pub async fn set_editor_text(&self, text: &str) -> Result<()> {
        let params = json!({ "text": text });
        self.call("ui.setEditorText", params).await.map(drop)
    }

    pub async fn editor_text(&self) -> Result<String> {
        let text: Option<String> = self.call_as("ui.getEditorText", json!({})).await?;
        Ok(text.unwrap_or_default())
    }

    /// Writes raw bytes to the terminal between frames; `false` when pi cannot write.
    pub async fn write_terminal(&self, data: &str, tmux_passthrough: bool) -> Result<bool> {
        let params = json!({ "data": data, "tmuxPassthrough": tmux_passthrough });
        let written: Option<bool> = self.call_as("ui.writeTerminal", params).await?;
        Ok(written.unwrap_or(false))
    }

    /// Starts a `ui.custom` panel; call `.open().await` on the builder.
    pub fn panel<I, S>(&self, lines_: I) -> PanelBuilder
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        PanelBuilder::new(self.clone(), lines(lines_))
    }
}
