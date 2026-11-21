use super::*;
use crate::config::codex_cloud;
use serde_json::json;

const HOOKS: &str = "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"mdm\"\n";

#[test]
fn the_first_plist_with_a_key_wins() {
    let user = json!({ MDM_CONFIG_KEY: BASE64.encode(HOOKS) });
    let device = json!({
        MDM_CONFIG_KEY: BASE64.encode("model = \"x\""),
        MDM_REQUIREMENTS_KEY: BASE64.encode("allow_managed_hooks_only = true"),
    });
    let docs = vec![("/u.plist".into(), user), ("/d.plist".into(), device)];
    let (file, config) = mdm_payload(&docs, MDM_CONFIG_KEY).unwrap();
    assert_eq!(file, PathBuf::from("/u.plist:config_toml_base64"));
    assert_eq!(config["hooks"]["Stop"][0]["hooks"][0]["command"], "mdm");
    let (_, requirements) = mdm_payload(&docs, MDM_REQUIREMENTS_KEY).unwrap();
    assert_eq!(requirements["allow_managed_hooks_only"], true);
    let bad = vec![("/b.plist".into(), json!({ MDM_CONFIG_KEY: 1 }))];
    assert!(mdm_payload(&bad, MDM_CONFIG_KEY).is_none());
}

#[cfg(target_os = "macos")]
#[test]
fn plutil_reads_a_plist_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let plist = dir.path().join(format!("{MDM_DOMAIN}.plist"));
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<plist version=\"1.0\"><dict><key>{MDM_CONFIG_KEY}</key><string>{}</string></dict></plist>\n",
        BASE64.encode(HOOKS)
    );
    std::fs::write(&plist, xml).unwrap();
    let docs = mdm_docs(&mdm_plists(dir.path()));
    let (_, config) = mdm_payload(&docs, MDM_CONFIG_KEY).unwrap();
    assert_eq!(config["hooks"]["Stop"][0]["hooks"][0]["command"], "mdm");
}

#[cfg(unix)]
#[test]
fn legacy_config_is_a_top_layer_behind_its_switch() {
    let dir = tempfile::tempdir().unwrap();
    let system = dir.path().join("etc");
    std::fs::create_dir_all(&system).unwrap();
    std::fs::write(system.join("managed_config.toml"), HOOKS).unwrap();
    let paths = Paths {
        claude_dir: dir.path().join("claude"),
        codex_home: dir.path().join("codex"),
        codex_system: Some(system),
        claude_policy: None,
        codex_mdm: None,
        temp_dir: dir.path().join("tmp"),
        agent_dir: None,
    };
    let mut options = Options::default();
    assert_eq!(ManagedSources::load(&paths, &options).top_configs.len(), 1);
    options.switches.insert(options::CODEX_LEGACY, false);
    assert!(
        ManagedSources::load(&paths, &options)
            .top_configs
            .is_empty()
    );
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn stop_hook(command: &str) -> String {
    format!("[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"{command}\"\n")
}

fn write_cache(home: &Path, config: &str, requirements: &str) {
    let fragment = |contents: &str| json!([{ "id": "f", "name": "F", "contents": contents }]);
    let payload = json!({
        "version": 1, "cached_at": "2026-11-21T00:00:00Z", "expires_at": "2999-01-01T00:00:00Z",
        "chatgpt_user_id": "u", "account_id": "a",
        "bundle": {
            "config_toml": { "enterprise_managed": fragment(config) },
            "requirements_toml": { "enterprise_managed": fragment(requirements) }
        }
    });
    let bytes = serde_json::to_vec(&payload).unwrap();
    let signature = BASE64.encode(codex_cloud::hmac_sha256(codex_cloud::HMAC_KEY, &bytes));
    let doc = json!({ "signed_payload": payload, "signature": signature });
    write(&home.join(codex_cloud::CACHE_FILE), &doc.to_string());
}

fn test_paths(base: &Path) -> Paths {
    Paths {
        claude_dir: base.join("claude"),
        codex_home: base.join("codex"),
        codex_system: Some(base.join("etc")),
        claude_policy: None,
        codex_mdm: None,
        temp_dir: base.join("tmp"),
        agent_dir: None,
    }
}

/// `<command>:<layer>` of each hook that `config::load` returns, with `off` switches off.
fn summaries(paths: &Paths, base: &Path, off: &[&str]) -> Vec<String> {
    let switches: serde_json::Map<String, Value> = off
        .iter()
        .map(|k| (k.split('/').nth(1).unwrap().to_owned(), json!(false)))
        .collect();
    let agent = base.join("agent");
    let options = json!({ "managed": switches }).to_string();
    write(&agent.join("ext/hooks.json"), &options);
    let input = crate::config::LoadInput {
        paths,
        agent_dir: &agent,
        cwd: &base.join("repo"),
        project_trusted: true,
    };
    let loaded = crate::config::load(&input);
    let hooks = loaded.hooks.iter();
    hooks
        .map(|h| format!("{}:{}", h.summary(), h.layer.name()))
        .collect()
}

#[test]
fn load_follows_codex_precedence_and_switches() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let paths = test_paths(base);
    let system = base.join("etc");
    let requirements = format!("allow_managed_hooks_only = true\n{}", stop_hook("req"));
    write(&system.join("requirements.toml"), &requirements);
    write(&system.join("config.toml"), &stop_hook("sys"));
    write(&paths.codex_home.join("config.toml"), &stop_hook("user"));
    let loose = "allow_managed_hooks_only = false\n[features]\nhooks = true\n";
    write_cache(&paths.codex_home, &stop_hook("cloud"), loose);
    let project = "[features]\nhooks = true\n";
    write(&base.join("repo/.codex/config.toml"), project);
    let run = |off: &[&str]| summaries(&paths, base, off);
    assert_eq!(
        run(&[]),
        ["req:managed", "sys:managed"],
        "a cloud allow_managed_hooks_only = false does not loosen requirements.toml"
    );
    assert_eq!(
        run(&[options::CODEX_REQUIREMENTS]),
        ["sys:managed", "cloud:global", "user:global"],
        "cloud hooks need trust"
    );
    assert_eq!(
        run(&[options::CODEX_REQUIREMENTS, options::CODEX_CLOUD]),
        ["sys:managed", "user:global"]
    );
    assert_eq!(run(&[options::CODEX_SYSTEM]), ["req:managed"]);
    if cfg!(unix) {
        write(
            &system.join("managed_config.toml"),
            "[features]\nhooks = false\n",
        );
        assert!(
            run(&[options::CODEX_REQUIREMENTS]).is_empty(),
            "managed_config.toml beats the project layer and a cloud hooks = true"
        );
        let off = [options::CODEX_REQUIREMENTS, options::CODEX_LEGACY];
        assert_eq!(run(&off).len(), 3);
    }
}

#[test]
fn cloud_requirements_apply_only_when_stricter() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let paths = test_paths(base);
    write(&base.join("etc/config.toml"), &stop_hook("sys"));
    write(&paths.codex_home.join("config.toml"), &stop_hook("user"));
    let run = |requirements: &str| {
        write_cache(&paths.codex_home, &stop_hook("cloud"), requirements);
        summaries(&paths, base, &[])
    };
    assert_eq!(run("allow_managed_hooks_only = true\n"), ["sys:managed"]);
    assert!(run("[features]\nhooks = false\n").is_empty());
    assert_eq!(
        run(&stop_hook("cloudreq")),
        [
            "sys:managed",
            "cloud:global",
            "cloudreq:global",
            "user:global"
        ],
        "requirements hooks need trust"
    );
}
