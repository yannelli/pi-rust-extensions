use super::*;
use crate::backends::backend;
use crate::testutil::TempDir;

const KEY: &str = "ts_live_0123456789abcdef";

#[test]
fn stores_owner_only_and_reads_back() {
    let dir = TempDir::new();
    let store = KeyStore::new(&dir.path().join("pi-typesafe"));
    let typesafe = backend("typesafe").unwrap();
    let liquid = backend("liquid").unwrap();
    assert_eq!(
        store.situation(&typesafe, &Env::fixed::<&str, &str>([])),
        KeySituation::Missing
    );
    store.store(KEY, &typesafe).unwrap();
    store.store(&format!(" {KEY}x "), &liquid).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&store.path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let text = fs::read_to_string(&store.path).unwrap();
    let raw: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(raw["version"], 1);
    assert_eq!(raw["keys"]["liquid"], format!("{KEY}x"));
    let stored = store.situation(&typesafe, &Env::fixed::<&str, &str>([]));
    assert_eq!(stored, KeySituation::Stored { key: KEY.into() });
    let env = Env::fixed([("TYPESAFE_API_KEY", " from-env ")]);
    assert_eq!(store.situation(&typesafe, &env).key(), Some("from-env"));
    assert!(store.clear("typesafe"));
    assert!(!store.clear("typesafe"));
    assert!(store.clear("liquid"));
    assert!(!store.path.exists());
}

#[cfg(unix)]
#[test]
fn refuses_group_readable_file() {
    use std::os::unix::fs::PermissionsExt;
    let dir = TempDir::new();
    let store = KeyStore::new(dir.path());
    store.store(KEY, &backend("typesafe").unwrap()).unwrap();
    fs::set_permissions(&store.path, fs::Permissions::from_mode(0o640)).unwrap();
    let situation = store.situation(&backend("typesafe").unwrap(), &Env::fixed::<&str, &str>([]));
    let KeySituation::Unusable { reason } = situation else {
        panic!("{situation:?}")
    };
    assert!(reason.contains("readable by other users"));
    assert!(!reason.contains(KEY));
}

#[test]
fn reads_legacy_shape_and_validates_keys() {
    let dir = TempDir::new();
    let store = KeyStore::new(dir.path());
    write_private(&store.path, &format!("{{\"apiKey\":\"{KEY}\"}}")).unwrap();
    assert_eq!(store.read("typesafe").unwrap().as_deref(), Some(KEY));
    assert!(normalize_key("short").is_err());
    assert!(normalize_key("has space in the middle!!").is_err());
    let message = normalize_key("\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}")
        .unwrap_err()
        .message;
    assert!(message.contains("nothing was saved"));
}
