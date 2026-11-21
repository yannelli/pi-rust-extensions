use std::path::Path;

use serde_json::json;

use super::*;
use crate::config::policy_sources::OsPolicy;
use crate::model::Layer;

struct Fixture {
    dir: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn write(&self, rel: &str, doc: &Value) {
        let path = self.dir.path().join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, doc.to_string()).unwrap();
    }

    fn remote(&self, doc: Value) {
        self.write("claude/remote-settings.json", &doc);
    }

    fn plist(&self, doc: Value) {
        self.write("mdm/com.anthropic.claudecode.plist", &doc);
    }

    fn files(&self, doc: Value) {
        self.write("policy/managed-settings.json", &doc);
    }
}

fn no_registry(_: Hive) -> PolicyRead {
    PolicyRead::Absent
}

fn hkcu_only(hive: Hive) -> PolicyRead {
    match hive {
        Hive::Hklm => PolicyRead::Absent,
        Hive::Hkcu => PolicyRead::Doc(json!({ "hooks": hooks("hkcu") })),
    }
}

fn unread_plist(path: &Path) -> PolicyRead {
    panic!("read {}", path.display())
}

fn unread_registry(hive: Hive) -> PolicyRead {
    panic!("read {}", hive.key())
}

/// The plist reader reads JSON, so tests write the plist as a JSON file.
fn fixture(read_registry: fn(Hive) -> PolicyRead) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().to_path_buf();
    let paths = Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        codex_system: None,
        claude_policy: Some(base.join("policy")),
        claude_os_policy: Some(OsPolicy {
            plists: vec![
                base.join("mdm/user/com.anthropic.claudecode.plist"),
                base.join("mdm/com.anthropic.claudecode.plist"),
            ],
            read_plist: sources::read_file,
            read_registry: Some(read_registry),
        }),
        temp_dir: base.join("tmp"),
        agent_dir: None,
    };
    Fixture { dir, paths }
}

fn hooks(command: &str) -> Value {
    json!({ "PreToolUse": [{ "hooks": [{ "type": "command", "command": command }] }] })
}

fn sources_of(docs: &[PolicyDoc]) -> Vec<PolicySource> {
    docs.iter().map(|doc| doc.source).collect()
}

#[test]
fn first_wins_takes_the_highest_source_with_a_policy_key() {
    let fx = fixture(no_registry);
    fx.remote(json!({ "wslInheritsWindowsSettings": true, "allowedHttpHookUrls": null }));
    fx.plist(json!({ "hooks": hooks("mdm") }));
    fx.files(json!({ "hooks": hooks("file"), "allowManagedHooksOnly": true }));
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    assert_eq!(sources_of(&docs), [PolicySource::Mdm]);
    assert_eq!(lock(&docs, "allowManagedHooksOnly"), None);
}

#[test]
fn merge_applies_every_admin_source_lowest_rank_first() {
    let fx = fixture(hkcu_only);
    fx.remote(json!({ "managedSourcesBehavior": "merge", "disableAllHooks": false }));
    fx.plist(json!({ "allowManagedHooksOnly": false }));
    fx.files(json!({ "allowManagedHooksOnly": true, "disableAllHooks": true }));
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    let expected = [PolicySource::Files, PolicySource::Mdm, PolicySource::Remote];
    assert_eq!(sources_of(&docs), expected);
    assert_eq!(lock(&docs, "allowManagedHooksOnly"), Some(true));
    assert_eq!(lock(&docs, "disableAllHooks"), Some(true));
    fx.files(json!({ "allowManagedHooksOnly": false }));
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    assert_eq!(lock(&docs, "allowManagedHooksOnly"), None);
    assert_eq!(lock(&docs, "disableAllHooks"), Some(false));
}

#[test]
fn merge_set_in_a_lower_source_is_ignored() {
    let fx = fixture(no_registry);
    fx.plist(json!({ "hooks": hooks("mdm") }));
    fx.files(json!({ "managedSourcesBehavior": "merge", "hooks": hooks("file") }));
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    assert_eq!(sources_of(&docs), [PolicySource::Mdm]);
}

#[test]
fn hkcu_applies_only_without_an_admin_document() {
    let fx = fixture(hkcu_only);
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    assert_eq!(sources_of(&docs), [PolicySource::Hkcu]);
    assert_eq!(docs[0].path, Hive::Hkcu.label());
    std::fs::create_dir_all(fx.dir.path().join("policy")).unwrap();
    std::fs::write(fx.dir.path().join("policy/managed-settings.json"), "{").unwrap();
    assert!(claude_policy_docs(&fx.paths, &Options::default()).is_empty());
}

#[test]
fn a_switched_off_source_is_not_read() {
    let mut fx = fixture(unread_registry);
    fx.paths.claude_os_policy.as_mut().unwrap().read_plist = unread_plist;
    fx.remote(json!({ "hooks": hooks("remote") }));
    fx.plist(json!({ "hooks": hooks("mdm") }));
    fx.files(json!({ "hooks": hooks("file") }));
    let mut options = Options::default();
    for key in [MDM_SWITCH, REGISTRY_SWITCH, SERVER_SWITCH] {
        options.switches.insert(key, false);
    }
    let docs = claude_policy_docs(&fx.paths, &options);
    assert_eq!(sources_of(&docs), [PolicySource::Files]);
    options.switches.insert(FILES_SWITCH, false);
    assert!(claude_policy_docs(&fx.paths, &options).is_empty());
}

#[test]
fn the_per_user_plist_wins_over_the_device_plist() {
    let fx = fixture(no_registry);
    fx.plist(json!({ "hooks": hooks("device") }));
    fx.write(
        "mdm/user/com.anthropic.claudecode.plist",
        &json!({ "hooks": hooks("user") }),
    );
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    assert_eq!(docs.len(), 1);
    assert!(
        docs[0]
            .path
            .ends_with("mdm/user/com.anthropic.claudecode.plist")
    );
    let user = fx
        .dir
        .path()
        .join("mdm/user/com.anthropic.claudecode.plist");
    std::fs::write(user, "{").unwrap();
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    assert_eq!(sources_of(&docs), [PolicySource::Mdm]);
    assert!(docs[0].path.ends_with("mdm/com.anthropic.claudecode.plist"));
}

#[test]
fn metadata_keys_do_not_deliver_policy() {
    let fx = fixture(no_registry);
    fx.remote(json!({ "$checksum": "abc" }));
    fx.files(json!({ "hooks": hooks("file") }));
    let docs = claude_policy_docs(&fx.paths, &Options::default());
    assert_eq!(sources_of(&docs), [PolicySource::Files]);
}

#[test]
fn invalid_hook_keys_fall_back_like_claude_code() {
    let doc = normalize(json!({
        "allowManagedHooksOnly": "false",
        "allowedHttpHookUrls": "https://hooks.example.com/*",
        "httpHookAllowedEnvVars": ["TOKEN"],
    }));
    assert_eq!(doc["allowManagedHooksOnly"], false);
    assert_eq!(doc["allowedHttpHookUrls"], json!([]));
    assert_eq!(doc["httpHookAllowedEnvVars"], json!(["TOKEN"]));
    assert_eq!(
        normalize(json!({ "allowManagedHooksOnly": 1 }))["allowManagedHooksOnly"],
        true
    );
}

fn load(fx: &Fixture) -> crate::config::Loaded {
    let cwd = fx.dir.path().join("repo");
    std::fs::create_dir_all(&cwd).unwrap();
    let input = crate::config::LoadInput {
        paths: &fx.paths,
        agent_dir: &fx.dir.path().join("agent"),
        cwd: &cwd,
        project_trusted: true,
    };
    crate::config::load(&input)
}

#[test]
fn server_cache_hooks_load_as_managed() {
    let fx = fixture(no_registry);
    fx.remote(json!({ "hooks": hooks("remote"), "allowManagedHooksOnly": true }));
    fx.write("claude/settings.json", &json!({ "hooks": hooks("user") }));
    let loaded = load(&fx);
    let commands: Vec<(&str, Layer)> = loaded
        .hooks
        .iter()
        .map(|def| (def.handler["command"].as_str().unwrap(), def.layer))
        .collect();
    assert_eq!(commands, [("remote", Layer::Managed)]);
    assert!(loaded.hooks[0].file.ends_with("remote-settings.json"));
}

#[test]
fn merge_turns_hooks_off_when_any_source_disables_them() {
    let fx = fixture(no_registry);
    fx.remote(json!({
        "managedSourcesBehavior": "merge",
        "disableAllHooks": false,
        "hooks": hooks("remote"),
    }));
    fx.files(json!({ "hooks": hooks("file"), "disableAllHooks": true }));
    assert!(load(&fx).hooks.is_empty());
}
