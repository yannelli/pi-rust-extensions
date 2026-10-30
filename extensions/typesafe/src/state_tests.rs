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
