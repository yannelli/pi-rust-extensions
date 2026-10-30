use crate::env::Env;
use crate::listing::{self, Status};

const SUBCOMMANDS: [&str; 12] = [
    "marketplace",
    "list",
    "info",
    "install",
    "reinstall",
    "update",
    "uninstall",
    "enable",
    "disable",
    "import",
    "browse",
    "help",
];
const MARKETPLACE_SUBCOMMANDS: [&str; 6] = [
    "add",
    "list",
    "update",
    "remove",
    "autoupdate",
    "noautoupdate",
];

fn marketplace_names(env: &Env) -> Vec<String> {
    listing::marketplaces(env, None)
        .unwrap_or_default()
        .into_iter()
        .map(|row| row.name)
        .collect()
}

fn plugin_ids(env: &Env, installed: Option<bool>) -> Vec<String> {
    let all = listing::plugins(env, None, None).unwrap_or_default();
    let mut ids: Vec<String> = all
        .iter()
        .flat_map(|mp| &mp.plugins)
        .filter(|row| match installed {
            Some(true) => row.enabled.is_some(),
            Some(false) => row.enabled.is_none() && row.status != Status::Unavailable,
            None => row.status != Status::Unavailable || row.enabled.is_some(),
        })
        .map(|row| row.id())
        .collect();
    ids.dedup();
    ids
}

/// Argument completions: each value is the whole argument text with the last word completed.
pub fn complete(env: &Env, prefix: &str) -> Vec<String> {
    let mut words: Vec<&str> = prefix.split_whitespace().collect();
    let current = if prefix.is_empty() || prefix.ends_with(char::is_whitespace) {
        ""
    } else {
        words.pop().unwrap_or("")
    };
    let candidates: Vec<String> = match words.as_slice() {
        [] => SUBCOMMANDS.iter().map(|s| (*s).to_owned()).collect(),
        [.., "--scope"] => vec!["user".into(), "project".into()],
        ["marketplace" | "mp"] => MARKETPLACE_SUBCOMMANDS
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        [
            "marketplace" | "mp",
            "update" | "remove" | "rm" | "autoupdate" | "noautoupdate",
        ] => marketplace_names(env),
        ["list"] => marketplace_names(env),
        ["install"] => plugin_ids(env, Some(false)),
        ["info"] => plugin_ids(env, None),
        ["uninstall" | "update" | "enable" | "disable" | "reinstall"] => {
            plugin_ids(env, Some(true))
        }
        [_, ..] if current.starts_with('-') => vec!["--scope".into()],
        _ => Vec::new(),
    };
    let head = words.join(" ");
    candidates
        .into_iter()
        .filter(|c| c.starts_with(current))
        .map(|c| {
            if head.is_empty() {
                c
            } else {
                format!("{head} {c}")
            }
        })
        .collect()
}
