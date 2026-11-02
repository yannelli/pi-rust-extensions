//! Release tooling: `cargo xtask plan | prepare | notes | check-tag`, ported from pi-rust.
//! Versions come from conventional commits since the last `v*` tag; see `usage()`.

mod changelog;
mod manifest;
mod version;

use std::path::{Path, PathBuf};
use std::process::Command;

use version::{Bump, Change, Commit, Version, classify, required_bump};

fn usage() -> String {
    "usage: cargo xtask <command>
  plan [--bump auto|patch|minor|major] [--github-output]   next version from conventional commits
  prepare <version>   set the workspace and catalog versions, refresh Cargo.lock, update CHANGELOG.md
  notes <version>     print the CHANGELOG.md section for a version
  check-tag <tag>     fail unless the tag matches the workspace version"
        .into()
}

fn root() -> PathBuf {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .join("../..")
        .canonicalize()
        .expect("xtask lives in crates/xtask")
}

fn read(name: &str) -> Result<String, String> {
    std::fs::read_to_string(root().join(name)).map_err(|error| format!("{name}: {error}"))
}

fn write(name: &str, text: &str) -> Result<(), String> {
    std::fs::write(root().join(name), text).map_err(|error| format!("{name}: {error}"))
}

fn git(args: &[&str]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root())
        .output()
        .map_err(|error| format!("git: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn workspace_version() -> Result<Version, String> {
    manifest::workspace_version(&read("Cargo.toml")?)
}

/// The nearest `v*` tag reachable from HEAD.
fn last_tag() -> Option<String> {
    git(&["describe", "--tags", "--abbrev=0", "--match", "v[0-9]*"])
        .ok()
        .map(|tag| tag.trim().to_string())
}

fn tag_exists(tag: &str) -> bool {
    git(&[
        "rev-parse",
        "--verify",
        "--quiet",
        &format!("refs/tags/{tag}"),
    ])
    .is_ok()
}

fn commits_since(tag: Option<&str>) -> Result<Vec<Commit>, String> {
    let range = tag
        .map(|tag| format!("{tag}..HEAD"))
        .unwrap_or_else(|| "HEAD".into());
    let log = git(&["log", "--no-merges", "--format=%H%x1f%s%x1f%b%x1e", &range])?;
    Ok(log
        .split('\x1e')
        .filter_map(|record| {
            let mut fields = record.trim_start_matches('\n').splitn(3, '\x1f');
            let hash = fields.next()?.trim().to_string();
            let subject = fields.next()?.trim().to_string();
            let body = fields.next().unwrap_or_default().trim().to_string();
            (!hash.is_empty()).then_some(Commit {
                hash,
                subject,
                body,
            })
        })
        .collect())
}

fn changes() -> Result<Vec<Change>, String> {
    Ok(commits_since(last_tag().as_deref())?
        .iter()
        .filter_map(classify)
        .collect())
}

/// The next version, or `None` when no commit since the last tag needs a release.
fn plan(bump: Option<Bump>) -> Result<(Option<Version>, usize), String> {
    let current = workspace_version()?;
    let changes = changes()?;
    // A version that was never tagged is released as-is, for example after a manual bump.
    if !tag_exists(&format!("v{current}")) {
        return Ok((Some(current), changes.len()));
    }
    let version = bump
        .or_else(|| required_bump(&changes))
        .map(|bump| current.bump(bump));
    Ok((version, changes.len()))
}

fn prepare(version: &Version) -> Result<(), String> {
    let changes = changes()?;
    write(
        "Cargo.toml",
        &manifest::set_workspace_version(&read("Cargo.toml")?, version)?,
    )?;
    write(
        "marketplace.json",
        &manifest::set_catalog_version(&read("marketplace.json")?, version)?,
    )?;
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["update", "--workspace", "--offline"])
        .current_dir(root())
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err("cargo update --workspace failed".into());
    }
    let text = std::fs::read_to_string(root().join("CHANGELOG.md")).unwrap_or_default();
    if changelog::section_notes(&text, version).is_none() {
        let date = git(&["log", "-1", "--format=%cs", "HEAD"])?
            .trim()
            .to_string();
        let section = changelog::render_section(version, &date, &changes);
        write("CHANGELOG.md", &changelog::insert_section(&text, &section))?;
    }
    println!("Prepared v{version}");
    Ok(())
}

fn write_github_output(pairs: &[(&str, String)]) -> Result<(), String> {
    use std::io::Write;
    let path =
        std::env::var("GITHUB_OUTPUT").map_err(|_| "GITHUB_OUTPUT is not set".to_string())?;
    let text: String = pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}\n"))
        .collect();
    std::fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|error| error.to_string())
}

fn run(args: &[String]) -> Result<(), String> {
    let flag_value = |name: &str| {
        args.iter()
            .position(|arg| arg == name)
            .and_then(|index| args.get(index + 1))
            .cloned()
    };
    let version_arg = |what: &str| -> Result<Version, String> {
        args.get(1)
            .ok_or(format!("{what} needs a version"))?
            .parse()
    };
    match args.first().map(String::as_str) {
        Some("plan") => {
            let bump = match flag_value("--bump").as_deref() {
                None | Some("auto") => None,
                Some(bump) => Some(bump.parse()?),
            };
            let (version, changes) = plan(bump)?;
            let version = version
                .map(|version| version.to_string())
                .unwrap_or_default();
            let pairs = [
                ("release", (!version.is_empty()).to_string()),
                (
                    "tag",
                    if version.is_empty() {
                        String::new()
                    } else {
                        format!("v{version}")
                    },
                ),
                ("prerelease", version.contains('-').to_string()),
                ("version", version),
                ("changes", changes.to_string()),
            ];
            for (key, value) in &pairs {
                println!("{key}={value}");
            }
            if args.iter().any(|arg| arg == "--github-output") {
                write_github_output(&pairs)?;
            }
            Ok(())
        }
        Some("prepare") => prepare(&version_arg("prepare")?),
        Some("notes") => {
            let version = version_arg("notes")?;
            let text = std::fs::read_to_string(root().join("CHANGELOG.md")).unwrap_or_default();
            let notes = changelog::section_notes(&text, &version)
                .ok_or(format!("CHANGELOG.md has no section for {version}"))?;
            println!("{notes}");
            Ok(())
        }
        Some("check-tag") => {
            let tag = args.get(1).ok_or("check-tag needs a tag")?;
            let tagged: Version = tag.parse()?;
            let current = workspace_version()?;
            if !tag.starts_with('v') || tagged != current {
                return Err(format!(
                    "tag {tag} does not match workspace version {current}"
                ));
            }
            println!("Tag {tag} matches the workspace version");
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&args) {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}
