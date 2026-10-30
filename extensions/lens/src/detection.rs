use std::path::{Path, PathBuf};

use crate::config::{Stage, ToolCommand};

pub fn language(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "rs" => Some("rust"),
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => Some("javascript"),
        "py" | "pyi" => Some("python"),
        "go" => Some("go"),
        _ => None,
    }
}

pub fn root(path: &Path, cwd: &Path, language: &str) -> PathBuf {
    let markers: &[&str] = match language {
        "rust" => &["Cargo.toml"],
        "javascript" => &["package.json", "tsconfig.json"],
        "python" => &["pyproject.toml", "ruff.toml", ".ruff.toml"],
        "go" => &["go.mod"],
        _ => &[],
    };
    path.parent()
        .into_iter()
        .flat_map(Path::ancestors)
        .find(|dir| markers.iter().any(|marker| dir.join(marker).is_file()))
        .unwrap_or(cwd)
        .to_path_buf()
}

fn command(name: &str, language: &str, stage: Stage, binary: &str, args: &[&str]) -> ToolCommand {
    ToolCommand {
        name: name.to_owned(),
        language: language.to_owned(),
        stage,
        command: binary.to_owned(),
        args: args.iter().map(|s| (*s).to_owned()).collect(),
        enabled: true,
    }
}

pub fn detect(root: &Path, language: &str) -> Vec<ToolCommand> {
    match language {
        "rust" if root.join("Cargo.toml").exists() => vec![
            command(
                "rustfmt",
                language,
                Stage::Format,
                "rustfmt",
                &["--edition", rust_edition(root), "{path}"],
            ),
            command(
                "cargo-check",
                language,
                Stage::Types,
                "cargo",
                &["check", "--message-format=json"],
            ),
            command(
                "clippy",
                language,
                Stage::Lint,
                "cargo",
                &["clippy", "--message-format=json", "--", "-D", "warnings"],
            ),
        ],
        "python" => vec![
            command(
                "ruff-format",
                language,
                Stage::Format,
                "ruff",
                &["format", "{path}"],
            ),
            command(
                "ruff",
                language,
                Stage::Lint,
                "ruff",
                &["check", "--output-format", "json", "{path}"],
            ),
        ],
        "go" if root.join("go.mod").exists() => vec![
            command("gofmt", language, Stage::Format, "gofmt", &["-w", "{path}"]),
            command("go-vet", language, Stage::Types, "go", &["vet", "./..."]),
        ],
        "javascript" => javascript(root),
        _ => vec![],
    }
}

/// Edition for rustfmt from the nearest `Cargo.toml`: its `package.edition`, the
/// workspace root's `workspace.package.edition` when inherited, else 2021.
pub fn rust_edition(root: &Path) -> &'static str {
    let mut manifests = root
        .ancestors()
        .filter_map(|dir| std::fs::read_to_string(dir.join("Cargo.toml")).ok())
        .map(|text| Manifest::parse(&text));
    let Some(nearest) = manifests.next() else {
        return "2021";
    };
    let edition = match nearest.package {
        Some(Edition::Set(edition)) => Some(edition),
        Some(Edition::Inherited) if nearest.workspace.is_some() => nearest.workspace.flatten(),
        Some(Edition::Inherited) => manifests.find_map(|item| item.workspace).flatten(),
        Some(Edition::Unset) => None,
        None => nearest.workspace.flatten(),
    };
    edition.unwrap_or("2021")
}

#[derive(Default)]
struct Manifest {
    package: Option<Edition>,
    workspace: Option<Option<&'static str>>,
}

enum Edition {
    Set(&'static str),
    Inherited,
    Unset,
}

impl Manifest {
    /// Line-based reader for `[section]` headers and one-line `key = value` pairs.
    /// Multi-line values and `[package]` written as an inline table are not read.
    fn parse(text: &str) -> Self {
        let mut manifest = Self::default();
        let mut section = "";
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or_default().trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.trim();
                match section {
                    "package" => manifest.package = manifest.package.or(Some(Edition::Unset)),
                    "workspace" | "workspace.package" => {
                        manifest.workspace = manifest.workspace.or(Some(None));
                    }
                    _ => {}
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key: String = key.chars().filter(|c| !c.is_whitespace()).collect();
            let value: String = value.chars().filter(|c| !c.is_whitespace()).collect();
            match (section, key.as_str()) {
                ("package", "edition.workspace") if value == "true" => {
                    manifest.package = Some(Edition::Inherited);
                }
                ("package", "edition") if value == "{workspace=true}" => {
                    manifest.package = Some(Edition::Inherited);
                }
                ("package", "edition") => {
                    manifest.package = Some(known(&value).map_or(Edition::Unset, Edition::Set));
                }
                ("workspace.package", "edition") => manifest.workspace = Some(known(&value)),
                _ => {}
            }
        }
        manifest
    }
}

fn known(value: &str) -> Option<&'static str> {
    let value = value.trim_matches(['"', '\'']);
    ["2015", "2018", "2021", "2024"]
        .into_iter()
        .find(|edition| *edition == value)
}

fn javascript(root: &Path) -> Vec<ToolCommand> {
    let package = std::fs::read(root.join("package.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .unwrap_or_default();
    let has = |name: &str| {
        ["dependencies", "devDependencies"]
            .iter()
            .any(|key| package[*key].get(name).is_some())
    };
    let mut tools = vec![];
    if has("prettier")
        || [".prettierrc", "prettier.config.js", ".prettierrc.json"]
            .iter()
            .any(|p| root.join(p).exists())
    {
        tools.push(command(
            "prettier",
            "javascript",
            Stage::Format,
            "prettier",
            &["--write", "{path}"],
        ));
    }
    if has("eslint")
        || [
            "eslint.config.js",
            "eslint.config.mjs",
            ".eslintrc.json",
            ".eslintrc.js",
        ]
        .iter()
        .any(|p| root.join(p).exists())
    {
        tools.push(command(
            "eslint",
            "javascript",
            Stage::Lint,
            "eslint",
            &["--format", "json", "{path}"],
        ));
    }
    if root.join("tsconfig.json").exists() {
        tools.push(command(
            "tsc",
            "javascript",
            Stage::Types,
            "tsc",
            &["--noEmit", "--pretty", "false"],
        ));
    }
    tools
}
