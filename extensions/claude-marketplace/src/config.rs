use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::env::Dirs;
use crate::jsonfile;

/// Desired state in `<scopeRoot>/ext/claude-marketplace.json`. Unknown keys are kept.
pub struct Config {
    dirs: Dirs,
    data: Map<String, Value>,
}

impl Config {
    pub fn load(dirs: &Dirs) -> Result<Config> {
        let data = jsonfile::read_object(&dirs.config_file())?;
        Ok(Config {
            dirs: dirs.clone(),
            data,
        })
    }

    pub fn save(&self) -> Result<()> {
        jsonfile::write(&self.dirs.config_file(), &Value::Object(self.data.clone()))
    }

    pub fn marketplaces(&self) -> Vec<(String, String, bool)> {
        let section = self.data.get("marketplaces").and_then(Value::as_object);
        section
            .into_iter()
            .flatten()
            .filter_map(|(name, entry)| {
                let source = entry.get("source")?.as_str()?.to_owned();
                let auto = entry
                    .get("autoUpdate")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                Some((name.clone(), source, auto))
            })
            .collect()
    }

    /// `(id, enabled)` of each plugin entry.
    pub fn plugins(&self) -> Vec<(String, bool)> {
        let section = self.data.get("plugins").and_then(Value::as_object);
        section
            .into_iter()
            .flatten()
            .map(|(id, entry)| {
                let enabled = entry.get("enabled").and_then(Value::as_bool);
                (id.clone(), enabled.unwrap_or(true))
            })
            .collect()
    }

    pub fn set_marketplace(&mut self, name: &str, source: &str) -> Result<()> {
        let section = jsonfile::object_entry(&mut self.data, "marketplaces")?;
        let auto = section
            .get(name)
            .and_then(|entry| entry.get("autoUpdate"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mut entry = section
            .get(name)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        entry.insert("source".into(), json!(source));
        entry.insert("autoUpdate".into(), json!(auto));
        section.insert(name.to_owned(), Value::Object(entry));
        Ok(())
    }

    pub fn set_auto_update(&mut self, name: &str, auto: bool) -> Result<bool> {
        let section = jsonfile::object_entry(&mut self.data, "marketplaces")?;
        match section.get_mut(name).and_then(Value::as_object_mut) {
            Some(entry) => {
                entry.insert("autoUpdate".into(), json!(auto));
                Ok(true)
            }
            None => Ok(false),
        }
    }

    pub fn remove_marketplace(&mut self, name: &str) -> Result<()> {
        jsonfile::object_entry(&mut self.data, "marketplaces")?.shift_remove(name);
        let suffix = format!("@{name}");
        let plugins = jsonfile::object_entry(&mut self.data, "plugins")?;
        plugins.retain(|id, _| !id.ends_with(&suffix));
        Ok(())
    }

    pub fn set_plugin(&mut self, id: &str, enabled: bool) -> Result<()> {
        let section = jsonfile::object_entry(&mut self.data, "plugins")?;
        let mut entry = section
            .get(id)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        entry.insert("enabled".into(), json!(enabled));
        section.insert(id.to_owned(), Value::Object(entry));
        Ok(())
    }

    pub fn remove_plugin(&mut self, id: &str) -> Result<()> {
        jsonfile::object_entry(&mut self.data, "plugins")?.shift_remove(id);
        Ok(())
    }
}
