use std::collections::BTreeMap;
use std::path::Path;

pub const BINARY: &str = "pi-rust-ext";
pub const CHECKSUMS: &str = "SHA256SUMS";
pub const TARGETS: [&str; 5] = [
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-pc-windows-msvc",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Archive {
    pub target: &'static str,
    pub file_name: String,
    pub sha256: String,
    pub size: u64,
}

pub fn archive_name(target: &str) -> String {
    let extension = if target.contains("-windows-") {
        "zip"
    } else {
        "tar.gz"
    };
    format!("{BINARY}-{target}.{extension}")
}

/// Parses `sha256sum` output: `<64 hex>  <name>` or `<64 hex> *<name>`, one file per line.
pub fn parse_checksums(text: &str) -> Result<BTreeMap<String, String>, String> {
    let mut sums = BTreeMap::new();
    for (number, line) in text
        .lines()
        .enumerate()
        .filter(|(_, l)| !l.trim().is_empty())
    {
        let invalid = || format!("{CHECKSUMS} line {}: invalid entry", number + 1);
        let (digest, rest) = line.split_once(' ').ok_or_else(invalid)?;
        let name = rest
            .strip_prefix(' ')
            .or_else(|| rest.strip_prefix('*'))
            .ok_or_else(invalid)?;
        if !is_sha256_hex(digest) {
            return Err(format!(
                "{CHECKSUMS} line {}: invalid SHA-256 digest",
                number + 1
            ));
        }
        if !is_plain_file_name(name) {
            return Err(format!(
                "{CHECKSUMS} line {}: invalid file name {name:?}",
                number + 1
            ));
        }
        if sums.insert(name.to_string(), digest.to_string()).is_some() {
            return Err(format!("{CHECKSUMS}: duplicate entry for {name}"));
        }
    }
    Ok(sums)
}

/// Reads `<dir>/SHA256SUMS` and returns one archive for each target, in `TARGETS` order.
pub fn collect(dir: &Path) -> Result<Vec<Archive>, String> {
    let sums_path = dir.join(CHECKSUMS);
    let text = std::fs::read_to_string(&sums_path)
        .map_err(|e| format!("read {}: {e}", sums_path.display()))?;
    let sums = parse_checksums(&text)?;
    TARGETS
        .iter()
        .map(|target| {
            let file_name = archive_name(target);
            let sha256 = sums
                .get(&file_name)
                .ok_or_else(|| format!("{CHECKSUMS} has no entry for {file_name}"))?
                .clone();
            let path = dir.join(&file_name);
            let metadata = std::fs::symlink_metadata(&path)
                .map_err(|e| format!("read {}: {e}", path.display()))?;
            if !metadata.is_file() {
                return Err(format!("{} is not a regular file", path.display()));
            }
            Ok(Archive {
                target,
                file_name,
                sha256,
                size: metadata.len(),
            })
        })
        .collect()
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn is_plain_file_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', '\0'])
        && !name.chars().any(char::is_control)
}
