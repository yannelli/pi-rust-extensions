use std::collections::BTreeSet;

use serde_json::{Map, Value, json};

use crate::archives::{Archive, BINARY};

pub const REPOSITORY: &str = "yannelli/pi-rust-extensions";
pub const SCHEMA_VERSION: u64 = 1;
pub const DEFAULT_PROTOCOL_VERSION: u64 = 1;
pub const DEFAULT_MIN_PI_VERSION: &str = "0.7.7";

const CATALOG_KEYS: [&str; 4] = [
    "schemaVersion",
    "protocolVersion",
    "minPiVersion",
    "extensions",
];
const EXTENSION_KEYS: [&str; 6] = [
    "name",
    "displayName",
    "description",
    "version",
    "args",
    "tags",
];

/// Builds the schema v1 index. `version` is the workspace version; the tag must be `v<version>`.
pub fn build_index(
    catalog: &Value,
    tag: &str,
    version: &str,
    archives: &[Archive],
) -> Result<Value, String> {
    if tag != format!("v{version}") || !is_version(version) {
        return Err(format!(
            "tag {tag:?} does not match workspace version {version:?}"
        ));
    }
    let catalog = as_object(catalog, "marketplace.json")?;
    reject_unknown_keys(catalog, &CATALOG_KEYS, "marketplace.json")?;
    if catalog.get("schemaVersion").and_then(Value::as_u64) != Some(SCHEMA_VERSION) {
        return Err(format!(
            "marketplace.json: schemaVersion must be {SCHEMA_VERSION}"
        ));
    }
    let protocol_version = match catalog.get("protocolVersion") {
        None => DEFAULT_PROTOCOL_VERSION,
        Some(value) => value
            .as_u64()
            .filter(|v| *v > 0)
            .ok_or("marketplace.json: protocolVersion must be a positive integer")?,
    };
    let min_pi_version = optional_string(catalog, "minPiVersion", "marketplace.json")?
        .unwrap_or(DEFAULT_MIN_PI_VERSION);
    if !is_version(min_pi_version) {
        return Err(format!(
            "marketplace.json: invalid minPiVersion {min_pi_version:?}"
        ));
    }

    let base = format!("https://github.com/{REPOSITORY}/releases/download/{tag}");
    let mut targets = Map::new();
    for archive in archives {
        let entry = json!({
            "url": format!("{base}/{}", archive.file_name),
            "sha256": archive.sha256,
            "size": archive.size,
        });
        if targets.insert(archive.target.to_string(), entry).is_some() {
            return Err(format!("duplicate archive for {}", archive.target));
        }
    }
    let platforms: Vec<&str> = archives.iter().map(|a| a.target).collect();

    let entries = catalog
        .get("extensions")
        .and_then(Value::as_array)
        .ok_or("marketplace.json: extensions must be an array")?;
    let mut names = BTreeSet::new();
    let mut extensions = Vec::with_capacity(entries.len());
    for entry in entries {
        let extension = build_extension(entry, version, &platforms)?;
        let name = extension["name"].as_str().unwrap_or_default().to_string();
        if !names.insert(name.clone()) {
            return Err(format!("marketplace.json: duplicate extension {name:?}"));
        }
        extensions.push(extension);
    }

    Ok(json!({
        "schemaVersion": SCHEMA_VERSION,
        "repository": REPOSITORY,
        "tag": tag,
        "protocolVersion": protocol_version,
        "minPiVersion": min_pi_version,
        "binaries": { BINARY: targets },
        "extensions": extensions,
    }))
}

/// The exact bytes that the release signs and uploads.
pub fn render(index: &Value) -> String {
    let mut text = serde_json::to_string_pretty(index).unwrap_or_default();
    text.push('\n');
    text
}

fn build_extension(entry: &Value, version: &str, platforms: &[&str]) -> Result<Value, String> {
    let entry = as_object(entry, "extension")?;
    let name = entry
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !is_extension_name(name) {
        return Err(format!("marketplace.json: invalid extension name {name:?}"));
    }
    let context = format!("extension {name:?}");
    reject_unknown_keys(entry, &EXTENSION_KEYS, &context)?;
    let display_name = required_string(entry, "displayName", &context)?;
    let description = required_string(entry, "description", &context)?;
    let extension_version = optional_string(entry, "version", &context)?.unwrap_or(version);
    if !is_version(extension_version) {
        return Err(format!("{context}: invalid version {extension_version:?}"));
    }
    let args = match entry.get("args") {
        None => vec![name.to_string()],
        Some(value) => string_list(value, &context, "args")?,
    };
    if args.first().map(String::as_str) != Some(name) {
        return Err(format!(
            "{context}: args must start with the extension name"
        ));
    }
    let tags = match entry.get("tags") {
        None => Vec::new(),
        Some(value) => string_list(value, &context, "tags")?,
    };
    Ok(json!({
        "name": name,
        "displayName": display_name,
        "description": description,
        "version": extension_version,
        "binary": BINARY,
        "args": args,
        "tags": tags,
        "platforms": platforms,
    }))
}

fn as_object<'a>(value: &'a Value, context: &str) -> Result<&'a Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{context}: expected a JSON object"))
}

fn reject_unknown_keys(
    object: &Map<String, Value>,
    known: &[&str],
    context: &str,
) -> Result<(), String> {
    match object.keys().find(|key| !known.contains(&key.as_str())) {
        Some(key) => Err(format!("{context}: unknown field {key:?}")),
        None => Ok(()),
    }
}

fn required_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<&'a str, String> {
    optional_string(object, key, context)?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{context}: {key} is required"))
}

fn optional_string<'a>(
    object: &'a Map<String, Value>,
    key: &str,
    context: &str,
) -> Result<Option<&'a str>, String> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value)),
        Some(_) => Err(format!("{context}: {key} must be a string")),
    }
}

fn string_list(value: &Value, context: &str, key: &str) -> Result<Vec<String>, String> {
    let invalid = || format!("{context}: {key} must be an array of non-empty strings");
    value
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .map(|item| {
            item.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(invalid)
        })
        .collect()
}

/// Extension names become directory names on install: lowercase ASCII, digits, inner hyphens.
pub fn is_extension_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'-'))
        && !name.starts_with('-')
        && !name.ends_with('-')
}

/// SemVer 2.0 with an optional pre-release (`1.2.3`, `1.2.3-rc.1`); build metadata is rejected.
pub fn is_version(value: &str) -> bool {
    let (core, pre) = match value.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (value, None),
    };
    let parts: Vec<&str> = core.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| is_numeric_identifier(part))
        && pre.is_none_or(|pre| pre.split('.').all(is_pre_release_identifier))
}

fn is_numeric_identifier(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) && (id == "0" || !id.starts_with('0'))
}

fn is_pre_release_identifier(id: &str) -> bool {
    let alphanumeric = id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-');
    let numeric = id.bytes().all(|b| b.is_ascii_digit());
    !id.is_empty() && alphanumeric && (!numeric || is_numeric_identifier(id))
}
