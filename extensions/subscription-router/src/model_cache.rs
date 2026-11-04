use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::family::Family;
use crate::filelock::FileLock;
use crate::fsutil::write_json;

#[derive(Deserialize, Serialize)]
pub(crate) struct ModelCache {
    version: u32,
    families: BTreeMap<String, Vec<Value>>,
}

impl Default for ModelCache {
    fn default() -> Self {
        Self {
            version: 1,
            families: BTreeMap::new(),
        }
    }
}

fn cache_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext/subscription-router/models.json")
}

impl ModelCache {
    pub(crate) fn load(agent_dir: &Path) -> Self {
        let path = cache_path(agent_dir);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                pi_ext_sdk::warn!("cannot read {}: {error}", path.display());
                return Self::default();
            }
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(cache) if cache.version == 1 => cache,
            Ok(cache) => {
                pi_ext_sdk::warn!(
                    "ignoring {} with unsupported version {}",
                    path.display(),
                    cache.version
                );
                Self::default()
            }
            Err(error) => {
                pi_ext_sdk::warn!("ignoring {}: {error}", path.display());
                Self::default()
            }
        }
    }

    pub(crate) fn models(&self, family: Family) -> &[Value] {
        self.families.get(family.base()).map_or(&[], Vec::as_slice)
    }

    pub(crate) async fn save(
        agent_dir: &Path,
        family: Family,
        models: Vec<Value>,
    ) -> std::io::Result<()> {
        let path = cache_path(agent_dir);
        let _lock = FileLock::acquire(&path).await?;
        let mut cache = Self::load(agent_dir);
        cache.families.insert(family.base().to_owned(), models);
        write_json(&path, &serde_json::to_value(cache)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sr-models-{name}-{}-{}",
            std::process::id(),
            crate::oauth::random_hex(8)
        ))
    }

    #[tokio::test]
    async fn saves_families_under_a_lock_and_replaces_only_the_updated_family() {
        let dir = temp_dir("save");
        let anthropic = vec![json!({ "provider": "anthropic", "id": "claude-opus-5-5" })];
        let codex = vec![json!({ "provider": "openai-codex", "id": "gpt-5.5" })];
        let (a, c) = tokio::join!(
            ModelCache::save(&dir, Family::Anthropic, anthropic.clone()),
            ModelCache::save(&dir, Family::Codex, codex.clone())
        );
        a.unwrap();
        c.unwrap();
        let cache = ModelCache::load(&dir);
        assert_eq!(cache.models(Family::Anthropic), anthropic);
        assert_eq!(cache.models(Family::Codex), codex);

        let fresh = vec![json!({ "provider": "anthropic", "id": "claude-sonnet-5" })];
        ModelCache::save(&dir, Family::Anthropic, fresh.clone())
            .await
            .unwrap();
        let cache = ModelCache::load(&dir);
        assert_eq!(cache.version, 1);
        assert_eq!(cache.models(Family::Anthropic), fresh);
        assert_eq!(cache.models(Family::Codex), codex);
        let path = cache_path(&dir);
        assert_eq!(
            std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
            1
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(path.parent().unwrap()), 0o700);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_invalid_and_unsupported_caches_have_no_models() {
        let dir = temp_dir("invalid");
        assert!(ModelCache::load(&dir).models(Family::Anthropic).is_empty());
        let path = cache_path(&dir);
        crate::fsutil::create_private_dir(path.parent().unwrap()).unwrap();
        for text in [
            "not json",
            r#"{"version": 1, "families": []}"#,
            r#"{"version": 2, "families": {"anthropic": [{"id": "unknown"}]}}"#,
        ] {
            std::fs::write(&path, text).unwrap();
            let cache = ModelCache::load(&dir);
            for family in Family::ALL {
                assert!(cache.models(family).is_empty());
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}
