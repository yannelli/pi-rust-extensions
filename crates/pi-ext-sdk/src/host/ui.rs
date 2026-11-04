use serde::Serialize;
use serde_json::{Value, json};

use super::{Host, lines, without_nulls};
use crate::error::Result;
use crate::panel::PanelBuilder;
use crate::theme::Theme;

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

/// The editor's prompt box layout.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PromptLayout {
    #[default]
    Frame,
    Caret,
    Harbor,
}

/// How chat messages and the editor look (`ui.setChatStyle`, feature `ui.chatStyle`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatStyle {
    /// Before the first line of each user message; the other lines get the same indent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_prefix: Option<String>,
    /// Before the first line of the text of each assistant message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assistant_prefix: Option<String>,
    /// Finished thinking blocks show the hidden thinking label.
    pub collapse_thinking: bool,
    /// Rounded editor corners, and side bars in the editor padding.
    pub rounded_editor: bool,
    /// A theme color token or `#rrggbb`; absent follows the thinking level and bash mode.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editor_border_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_layout: Option<PromptLayout>,
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
        self.input_with(
            title,
            InputOptions {
                placeholder: placeholder.map(str::to_owned),
                ..InputOptions::default()
            },
        )
        .await
    }

    /// `ui.input` with a placeholder, initial text, or masked (`secret`) entry.
    pub async fn input_with(&self, title: &str, options: InputOptions) -> Result<Option<String>> {
        self.call_as("ui.input", options.params(title)).await
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

    /// The text of the working indicator; `None` restores pi's own (feature `ui.working`).
    pub async fn set_working_message(&self, message: Option<&str>) -> Result<()> {
        let params = without_nulls(json!({ "message": message }));
        self.call("ui.setWorkingMessage", params).await.map(drop)
    }

    /// Frames of the working indicator, drawn as they are; `None` restores pi's spinner.
    pub async fn set_working_indicator<S: AsRef<str>>(
        &self,
        frames: Option<&[S]>,
        interval_ms: Option<u64>,
    ) -> Result<()> {
        let frames: Option<Vec<&str>> =
            frames.map(|frames| frames.iter().map(AsRef::as_ref).collect());
        let params = without_nulls(json!({ "frames": frames, "intervalMs": interval_ms }));
        self.call("ui.setWorkingIndicator", params).await.map(drop)
    }

    pub async fn set_working_visible(&self, visible: bool) -> Result<()> {
        let params = json!({ "visible": visible });
        self.call("ui.setWorkingVisible", params).await.map(drop)
    }

    /// The label of hidden thinking blocks; `None` restores pi's own.
    pub async fn set_hidden_thinking_label(&self, label: Option<&str>) -> Result<()> {
        let params = without_nulls(json!({ "label": label }));
        self.call("ui.setHiddenThinkingLabel", params)
            .await
            .map(drop)
    }

    /// Message prefixes, collapsed thinking, and editor styling; replaces the last style.
    pub async fn set_chat_style(&self, style: &ChatStyle) -> Result<()> {
        let params = serde_json::to_value(style).unwrap_or_default();
        self.call("ui.setChatStyle", params).await.map(drop)
    }

    /// The theme the TUI draws with (feature `ui.getTheme`).
    pub async fn theme(&self) -> Result<Theme> {
        self.call_as("ui.getTheme", json!({})).await
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

/// Options of `Host::input_with`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputOptions {
    pub placeholder: Option<String>,
    /// The initial text.
    pub prefill: Option<String>,
    /// Show `*` for each character in `tui` mode.
    pub secret: bool,
}

impl InputOptions {
    fn params(&self, title: &str) -> Value {
        let mut params = without_nulls(
            json!({ "title": title, "placeholder": self.placeholder, "prefill": self.prefill }),
        );
        if self.secret {
            params["secret"] = json!(true);
        }
        params
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;

    #[test]
    fn chat_style_omits_optional_fields_and_serializes_layouts() {
        assert_eq!(
            serde_json::to_value(ChatStyle::default()).unwrap(),
            json!({ "collapseThinking": false, "roundedEditor": false })
        );
        for (layout, name) in [
            (PromptLayout::Frame, "frame"),
            (PromptLayout::Caret, "caret"),
            (PromptLayout::Harbor, "harbor"),
        ] {
            let style = ChatStyle {
                editor_border_color: Some("borderMuted".into()),
                prompt_layout: Some(layout),
                ..ChatStyle::default()
            };
            assert_eq!(
                serde_json::to_value(style).unwrap(),
                json!({ "collapseThinking": false, "roundedEditor": false,
                    "editorBorderColor": "borderMuted", "promptLayout": name })
            );
        }
    }

    #[test]
    fn input_params_carry_prefill_and_secret() {
        assert_eq!(InputOptions::default().params("T"), json!({ "title": "T" }));
        let options = InputOptions {
            prefill: Some("ab".into()),
            secret: true,
            ..InputOptions::default()
        };
        assert_eq!(
            options.params("Token"),
            json!({ "title": "Token", "prefill": "ab", "secret": true })
        );
    }
}
