//! The workspace version in `Cargo.toml` and the extension versions in `marketplace.json`.

use serde_json::Value;

use crate::version::Version;

/// The `version` of `[workspace.package]`.
pub fn workspace_version(manifest: &str) -> Result<Version, String> {
    let mut in_package = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[workspace.package]";
        } else if in_package && trimmed.starts_with("version") && trimmed.contains('=') {
            let value = trimmed.split_once('=').map_or("", |(_, v)| v);
            return value.trim().trim_matches('"').parse();
        }
    }
    Err("no version in [workspace.package]".into())
}

/// `manifest` with the `[workspace.package]` version set to `version`.
pub fn set_workspace_version(manifest: &str, version: &Version) -> Result<String, String> {
    let mut in_package = false;
    let mut replaced = false;
    let lines: Vec<String> = manifest
        .lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with('[') {
                in_package = trimmed == "[workspace.package]";
            } else if in_package
                && !replaced
                && trimmed.starts_with("version")
                && trimmed.contains('=')
            {
                replaced = true;
                return format!("version = \"{version}\"");
            }
            line.to_string()
        })
        .collect();
    if !replaced {
        return Err("no version in [workspace.package]".into());
    }
    Ok(lines.join("\n") + "\n")
}

/// `catalog` with every extension `version` set to `version`, in the same layout.
pub fn set_catalog_version(catalog: &str, version: &Version) -> Result<String, String> {
    let mut value: Value =
        serde_json::from_str(catalog).map_err(|error| format!("marketplace.json: {error}"))?;
    let extensions = value["extensions"]
        .as_array_mut()
        .ok_or("marketplace.json has no extensions array")?;
    for extension in extensions {
        if let Some(entry) = extension
            .as_object_mut()
            .filter(|entry| entry.contains_key("version"))
        {
            entry.insert("version".into(), Value::String(version.to_string()));
        }
    }
    serde_json::to_string_pretty(&value)
        .map(|text| text + "\n")
        .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = "[workspace]\nmembers = [\"crates/*\"]\n\n[workspace.package]\nversion = \"0.5.0\"\nedition = \"2024\"\n\n[profile.release]\nversion = \"9\"\n";

    #[test]
    fn reads_and_sets_the_workspace_version() {
        assert_eq!(workspace_version(MANIFEST).unwrap().to_string(), "0.5.0");
        let next: Version = "0.5.1".parse().unwrap();
        let updated = set_workspace_version(MANIFEST, &next).unwrap();
        assert_eq!(
            updated,
            MANIFEST.replace("version = \"0.5.0\"", "version = \"0.5.1\"")
        );
        assert!(workspace_version("[package]\nversion = \"1.0.0\"\n").is_err());
    }

    #[test]
    fn sets_catalog_versions_and_keeps_the_layout() {
        let catalog = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../marketplace.json"
        ))
        .unwrap();
        let current = workspace_version(
            &std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../Cargo.toml"))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            set_catalog_version(&catalog, &current).unwrap(),
            catalog,
            "marketplace.json is not in serde_json pretty layout"
        );
        let next: Version = "9.9.9".parse().unwrap();
        let updated = set_catalog_version(&catalog, &next).unwrap();
        assert_eq!(
            updated.matches("\"version\": \"9.9.9\"").count(),
            catalog.matches("\"version\": \"").count()
        );
    }
}
