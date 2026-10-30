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
                &["--edition", "2024", "{path}"],
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
