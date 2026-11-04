use std::io::Write;

use super::*;

fn meta(id: &str) -> Meta {
    Meta {
        id: id.into(),
        tool: "bash".into(),
        tool_call_id: "call_1".into(),
        session_id: Some("s-1".into()),
        chars: 3,
        created: 10,
        ..Meta::default()
    }
}

#[test]
fn saves_loads_lists_and_removes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(root(dir.path()));
    assert!(store.dir.ends_with("ext/tool-results/store"));
    store.save(&meta("tr-1"), "one").unwrap();
    store
        .save(
            &Meta {
                created: 5,
                ..meta("tr-2")
            },
            "two!",
        )
        .unwrap();
    assert_eq!(store.load("tr-1").as_deref(), Some("one"));
    assert_eq!(store.meta("tr-2").unwrap().bytes, 4);
    let ids: Vec<String> = store.list().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, ["tr-2", "tr-1"]);
    assert_eq!(store.load("../x"), None);
    assert!(store.remove("tr-1"));
    assert_eq!(store.load("tr-1"), None);
    assert!(!store.remove("tr-1"));
}

#[test]
fn ids_depend_on_the_call_and_the_content() {
    let (a, b) = (hash("a"), hash("b"));
    assert!(a.starts_with("sha256:") && a.len() == 71);
    assert_eq!(result_id("call_1", &a), result_id("call_1", &a));
    assert_ne!(result_id("call_1", &a), result_id("call_1", &b));
    assert_ne!(result_id("call_1", &a), result_id("call_2", &a));
    let id = result_id("x", &a);
    assert!(valid(&id) && id.len() == 15, "{id}");
}

#[test]
fn a_crash_during_a_write_leaves_no_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tr-1.txt");
    let failed = write_atomic_with(&path, |file| {
        file.write_all(b"half of the res")?;
        Err(std::io::Error::other("disk full"))
    });
    assert!(failed.is_err());
    assert!(!path.exists());
    let left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().flatten().collect();
    assert!(left.is_empty(), "{left:?}");
    std::fs::write(&path, "old").unwrap();
    let failed = write_atomic_with(&path, |file| {
        file.write_all(b"new and lon")?;
        Err(std::io::Error::other("killed"))
    });
    assert!(failed.is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "old",
        "the old file stays whole"
    );
}

#[test]
fn a_text_without_its_record_is_not_a_stored_result() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(root(dir.path()));
    std::fs::create_dir_all(&store.dir).unwrap();
    std::fs::write(store.dir.join("tr-9.txt"), "written before a crash").unwrap();
    assert_eq!(store.load("tr-9"), None);
    assert!(store.list().is_empty());
}
