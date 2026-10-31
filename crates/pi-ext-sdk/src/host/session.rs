use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::{Host, without_nulls};
use crate::error::Result;
use crate::snapshot::Model;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DeliverAs {
    Steer,
    FollowUp,
    NextTurn,
}

/// A custom message for `session.sendMessage`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomMessage {
    pub custom_type: String,
    pub content: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deliver_as: Option<DeliverAs>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trigger_turn: Option<bool>,
}

impl CustomMessage {
    /// `content` is a string or an array of content blocks.
    pub fn new(custom_type: impl Into<String>, content: impl Into<Value>) -> Self {
        Self {
            custom_type: custom_type.into(),
            content: content.into(),
            display: None,
            details: None,
            deliver_as: None,
            trigger_turn: None,
        }
    }

    pub fn display(mut self, display: bool) -> Self {
        self.display = Some(display);
        self
    }

    pub fn details(mut self, details: impl Serialize) -> Self {
        self.details = serde_json::to_value(details).ok();
        self
    }

    pub fn deliver_as(mut self, deliver_as: DeliverAs) -> Self {
        self.deliver_as = Some(deliver_as);
        self
    }

    pub fn trigger_turn(mut self, trigger: bool) -> Self {
        self.trigger_turn = Some(trigger);
        self
    }
}

/// `session.*`, `model.*`, `thinking.*`, `tools.*`, `commands.getAll`, `flags.get`, `settings.get`.
impl Host {
    async fn call_list<T: DeserializeOwned>(&self, method: &str, params: Value) -> Result<Vec<T>> {
        let list: Option<Vec<T>> = self.call_as(method, params).await?;
        Ok(list.unwrap_or_default())
    }

    pub async fn send_message(&self, message: CustomMessage) -> Result<()> {
        let params = serde_json::to_value(message)?;
        self.call("session.sendMessage", params).await.map(drop)
    }

    pub async fn send_user_message(
        &self,
        content: impl Into<Value>,
        deliver_as: Option<DeliverAs>,
    ) -> Result<()> {
        let params = json!({ "content": content.into(), "deliverAs": deliver_as });
        let params = without_nulls(params);
        self.call("session.sendUserMessage", params).await.map(drop)
    }

    /// Appends a `custom` session entry with `customType` and `data`.
    pub async fn append_entry<T: Serialize>(&self, custom_type: &str, data: &T) -> Result<()> {
        let params = json!({ "customType": custom_type, "data": data });
        self.call("session.appendEntry", params).await.map(drop)
    }

    pub async fn entries(&self) -> Result<Vec<Value>> {
        self.call_list("session.getEntries", json!({})).await
    }

    /// Entries from the root to the leaf (or to `from_id`).
    pub async fn branch(&self, from_id: Option<&str>) -> Result<Vec<Value>> {
        let params = without_nulls(json!({ "fromId": from_id }));
        self.call_list("session.getBranch", params).await
    }

    /// `data` of the `custom` entries with `customType` on the current branch, oldest first.
    /// Entries whose data does not deserialize into `T` are skipped.
    pub async fn entries_of<T: DeserializeOwned>(&self, custom_type: &str) -> Result<Vec<T>> {
        let branch = self.branch(None).await?;
        Ok(custom_data(branch, custom_type))
    }

    pub async fn set_session_name(&self, name: &str) -> Result<()> {
        self.call("session.setName", json!({ "name": name }))
            .await
            .map(drop)
    }

    pub async fn session_name(&self) -> Result<Option<String>> {
        self.call_as("session.getName", json!({})).await
    }

    pub async fn model(&self) -> Result<Option<Model>> {
        self.call_as("model.get", json!({})).await
    }

    /// Switches the model; `false` when pi has no such model or no key for it.
    pub async fn set_model(&self, provider: &str, id: &str) -> Result<bool> {
        let params = json!({ "provider": provider, "id": id });
        let switched: Option<bool> = self.call_as("model.set", params).await?;
        Ok(switched.unwrap_or(false))
    }

    pub async fn thinking_level(&self) -> Result<Option<String>> {
        self.call_as("thinking.get", json!({})).await
    }

    pub async fn set_thinking_level(&self, level: &str) -> Result<()> {
        self.call("thinking.set", json!({ "level": level }))
            .await
            .map(drop)
    }

    pub async fn active_tools(&self) -> Result<Vec<String>> {
        self.call_list("tools.getActive", json!({})).await
    }

    pub async fn set_active_tools<S: AsRef<str>>(&self, names: &[S]) -> Result<()> {
        let names: Vec<&str> = names.iter().map(AsRef::as_ref).collect();
        self.call("tools.setActive", json!({ "names": names }))
            .await
            .map(drop)
    }

    pub async fn all_tools(&self) -> Result<Vec<Value>> {
        self.call_list("tools.getAll", json!({})).await
    }

    pub async fn all_commands(&self) -> Result<Vec<Value>> {
        self.call_list("commands.getAll", json!({})).await
    }

    /// The value of a flag of this extension, or `None` when unset.
    pub async fn flag<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>> {
        self.call_as("flags.get", json!({ "name": name })).await
    }

    /// The merged global and project settings under `namespace`; `None` when absent.
    pub async fn settings<T: DeserializeOwned>(&self, namespace: &str) -> Result<Option<T>> {
        self.call_as("settings.get", json!({ "namespace": namespace }))
            .await
    }

    /// All merged global and project settings.
    pub async fn all_settings(&self) -> Result<Value> {
        self.call("settings.get", json!({})).await
    }

    /// Writes `value` as the top-level key `namespace` of the `scope` settings file, keeping the
    /// other keys; `Value::Null` removes it. pi refuses its own keys (-32602) and the project
    /// scope of an untrusted project (-32003).
    pub async fn set_settings(
        &self,
        namespace: &str,
        value: impl serde::Serialize,
        scope: crate::render::SettingsScope,
    ) -> Result<()> {
        let value = serde_json::to_value(value)?;
        let params = json!({ "namespace": namespace, "value": value, "scope": scope });
        self.call("settings.set", params).await.map(drop)
    }

    /// The key text of app keybindings, for example `"app.tools.expand"` to `"ctrl+o"` (upstream
    /// `keyText` of renderers). Every app keybinding when `ids` is empty.
    pub async fn keybindings(
        &self,
        ids: &[&str],
    ) -> Result<std::collections::BTreeMap<String, String>> {
        let params = if ids.is_empty() {
            json!({})
        } else {
            json!({ "ids": ids })
        };
        self.call_as("keybindings.get", params).await
    }
}

pub(crate) fn custom_data<T: DeserializeOwned>(entries: Vec<Value>, custom_type: &str) -> Vec<T> {
    entries
        .into_iter()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("custom"))
        .filter(|entry| entry.get("customType").and_then(Value::as_str) == Some(custom_type))
        .filter_map(|mut entry| entry.get_mut("data").map(Value::take))
        .filter_map(|data| serde_json::from_value(data).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_custom_entries() {
        let entries = vec![
            json!({ "type": "message", "customType": "todo", "data": 1 }),
            json!({ "type": "custom", "customType": "todo", "data": 2 }),
            json!({ "type": "custom", "customType": "other", "data": 3 }),
            json!({ "type": "custom", "customType": "todo", "data": "bad" }),
            json!({ "type": "custom", "customType": "todo", "data": 4 }),
        ];
        assert_eq!(custom_data::<i64>(entries, "todo"), vec![2, 4]);
    }
}
