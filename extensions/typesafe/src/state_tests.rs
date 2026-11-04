use super::*;
use crate::testutil::TempDir;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn project(dir: &TempDir, text: &str, trusted: bool) -> Project {
    write(&dir.path().join("work/.pi/ext/typesafe.json"), text);
    Project::load(&dir.path().join("work"), trusted)
}

#[test]
fn gate_precedence() {
    let dir = TempDir::new();
    let agent = dir.path().join("agent");
    write(
        &agent.join("ext/typesafe.json"),
        r#"{ "enabled": true, "maxRequestsPerDay": 9 }"#,
    );
    let state = State::new(Env::fixed::<&str, &str>([]), &agent);
    assert!(state.enabled());
    let off = State::new(Env::fixed([("PI_TYPESAFE_ENABLED", "0")]), &agent);
    assert!(!off.enabled());
    let untrusted = project(
        &dir,
        r#"{ "enabled": false, "maxRequestsPerDay": 3 }"#,
        false,
    );
    assert!(untrusted.ignored_enable());
    state.reset_session(&untrusted);
    assert!(state.enabled());
    assert_eq!(state.lock().caps.max_requests_per_day, Some(3));
    state.reset_session(&project(&dir, r#"{ "enabled": false }"#, true));
    assert!(!state.enabled());
    assert_eq!(state.lock().caps.max_requests_per_day, Some(9));
}

#[test]
fn saving_config_keeps_other_keys_and_applies_the_saved_model() {
    let dir = TempDir::new();
    let path = dir.path().join("ext/typesafe.json");
    write(
        &path,
        r#"{ "maxRequestsPerDay": 9, "model": "old", "priceUsdPerMtok": 1 }"#,
    );
    let changes = [
        ("model", serde_json::json!("new")),
        ("priceUsdPerMtok", Value::Null),
    ];
    assert_eq!(Config::save(dir.path(), &changes).unwrap(), path);
    let saved = Config::load(dir.path());
    assert_eq!(saved.model.as_deref(), Some("new"));
    assert_eq!(saved.price_usd_per_mtok, None);
    assert_eq!(saved.max_requests_per_day, Some(9));
    write(&path, "not json");
    assert!(Config::save(dir.path(), &changes).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");

    let state = State::new(Env::fixed::<&str, &str>([]), dir.path());
    state.set_model("jev-1.14", Some(0.05));
    assert_eq!(state.model().as_deref(), Some("jev-1.14"));
    assert!(state.consent().0.contains("as model \"jev-1.14\""));
}

#[test]
fn backend_and_consent() {
    let dir = TempDir::new();
    let state = State::new(Env::fixed::<&str, &str>([]), dir.path());
    assert_eq!(state.consent(), (FIXED_CONSENT.to_owned(), false));
    let liquid = State::new(Env::fixed([("PI_TYPESAFE_BACKEND", "liquid")]), dir.path());
    let (text, names) = liquid.consent();
    assert!(names && text.starts_with("Submitted state and questions will be sent to api.liquid.ai (Liquid AI) as model \"d1:free\""));
    let bad = State::new(Env::fixed([("PI_TYPESAFE_BACKEND", "nope")]), dir.path());
    assert!(
        bad.backend_error
            .as_deref()
            .unwrap()
            .starts_with("Unknown judgment backend \"nope\"")
    );
    assert_eq!(bad.backend.name, "typesafe");
}
