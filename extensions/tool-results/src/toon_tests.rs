//! Expected output from Appendix A and §7 of the TOON spec v4.4.

use serde_json::json;

use super::*;

fn check(value: serde_json::Value, expected: &str) {
    assert_eq!(encode(&value), expected, "{value}");
}

#[test]
fn objects_and_nesting() {
    check(
        json!({ "id": 123, "name": "Ada", "active": true }),
        "id: 123\nname: Ada\nactive: true",
    );
    check(
        json!({ "user": { "id": 123, "name": "Ada" } }),
        "user:\n  id: 123\n  name: Ada",
    );
    check(json!({ "empty": {} }), "empty:");
    check(json!({}), "");
}

#[test]
fn primitive_arrays_and_empty_arrays() {
    check(
        json!({ "tags": ["admin", "ops", "dev"] }),
        "tags[3]: admin,ops,dev",
    );
    check(json!({ "tags": [] }), "tags: []");
    check(json!([]), "[]");
    check(json!(["a", 1, true]), "[3]: a,1,true");
    check(json!({ "tags": ["🎉", "🎊", "🎈"] }), "tags[3]: 🎉,🎊,🎈");
}

#[test]
fn tabular_arrays() {
    let items = json!({ "items": [
        { "sku": "A1", "qty": 2, "price": 9.99 },
        { "sku": "B2", "qty": 1, "price": 14.5 }
    ]});
    check(items, "items[2]{sku,qty,price}:\n  A1,2,9.99\n  B2,1,14.5");
    let orders = json!({ "orders": [
        { "id": 1, "customer": { "name": "Ada", "country": "DK" }, "total": 99 },
        { "id": 2, "customer": { "name": "Bob", "country": "UK" }, "total": 149 }
    ]});
    check(
        orders,
        "orders[2]{id,customer{name,country},total}:\n  1,Ada,DK,99\n  2,Bob,UK,149",
    );
    let root = json!([{ "id": 1, "name": "Ada" }, { "name": "Bob", "id": 2 }]);
    check(root, "[2]{id,name}:\n  1,Ada\n  2,Bob");
    let links = json!({ "links": [
        { "id": 1, "url": "http://a:b" },
        { "id": 2, "url": "https://example.com?q=a:b" }
    ]});
    check(
        links,
        "links[2]{id,url}:\n  1,\"http://a:b\"\n  2,\"https://example.com?q=a:b\"",
    );
}

#[test]
fn keyed_tabular_objects() {
    let users = json!({ "users": {
        "alice": { "age": 30, "city": "Berlin" },
        "bob": { "age": 25, "city": "Oslo" }
    }});
    check(
        users,
        "users[2:]{age,city}:\n  alice: 30,Berlin\n  bob: 25,Oslo",
    );
    let root =
        json!({ "alice": { "age": 30, "city": "Berlin" }, "bob": { "age": 25, "city": "Oslo" } });
    check(root, "[2:]{age,city}:\n  alice: 30,Berlin\n  bob: 25,Oslo");
    check(json!({ "one": { "a": 1 } }), "one:\n  a: 1");
}

#[test]
fn list_forms() {
    check(
        json!({ "items": [1, { "a": 1 }, "text"] }),
        "items[3]:\n  - 1\n  - a: 1\n  - text",
    );
    let items = json!({ "items": [
        { "id": 1, "name": "First" },
        { "id": 2, "name": "Second", "extra": true }
    ]});
    check(
        items,
        "items[2]:\n  - id: 1\n    name: First\n  - id: 2\n    name: Second\n    extra: true",
    );
    check(
        json!({ "pairs": [[1, 2], [3, 4]] }),
        "pairs[2]:\n  - [2]: 1,2\n  - [2]: 3,4",
    );
    let nested = json!({ "items": [{ "users": [{ "id": 1, "name": "Ada" }, { "id": 2, "name": "Bob" }], "status": "active" }] });
    check(
        nested,
        "items[1]:\n  - users[2]{id,name}:\n      1,Ada\n      2,Bob\n    status: active",
    );
    check(
        json!({ "x": [{}, [], [{ "a": 1 }]] }),
        "x[3]:\n  -\n  - [0]:\n  - [1]:\n    - a: 1",
    );
}

#[test]
fn deep_nesting_and_edge_cases() {
    let deep = json!({ "root": { "level1": { "level2": { "level3": { "items": [{ "id": 1, "val": "a" }, { "id": 2, "val": "b" }] } } } } });
    check(
        deep,
        "root:\n  level1:\n    level2:\n      level3:\n        items[2]{id,val}:\n          1,a\n          2,b",
    );
    check(json!({ "name": "" }), "name: \"\"");
    check(
        json!({ "version": "123", "enabled": "true" }),
        "version: \"123\"\nenabled: \"true\"",
    );
    check(
        json!({ "message": "Hello 世界 👋" }),
        "message: Hello 世界 👋",
    );
    check(
        json!({ "bignum": 9007199254740992u64, "decimal": 0.3333333333333333 }),
        "bignum: 9007199254740992\ndecimal: 0.3333333333333333",
    );
    check(json!({ "my-key": [1, 2, 3] }), "\"my-key\"[3]: 1,2,3");
    let quoted = json!({ "x-items": [{ "id": 1 }, { "id": 2, "label": "archived" }] });
    check(
        quoted,
        "\"x-items\"[2]:\n  - id: 1\n  - id: 2\n    label: archived",
    );
}

#[test]
fn quoting_rules() {
    for s in [
        "",
        " a",
        "a ",
        "true",
        "false",
        "null",
        "42",
        "-3.14",
        "05",
        "+1",
        "1e-6",
        "a:b",
        "say \"hi\"",
        "a\\b",
        "[x]",
        "{y}",
        "a,b",
        "-x",
        "#tag",
        "line\nbreak",
    ] {
        assert!(string(s).starts_with('"'), "{s:?} must be quoted");
    }
    for s in ["hello world", "Ada", "1.2.3", "e5", "a-b", "x#"] {
        assert_eq!(string(s), s, "{s:?} stays bare");
    }
    assert_eq!(string("tab\there"), "\"tab\\there\"");
    assert_eq!(string("bell\u{7}"), "\"bell\\u0007\"");
    assert_eq!(encode_key("user.name"), "user.name");
    assert_eq!(encode_key("2key"), "\"2key\"");
}

#[test]
fn canonical_numbers() {
    let n = |v: serde_json::Value| number(v.as_number().unwrap());
    assert_eq!(n(json!(1.5000)), "1.5");
    assert_eq!(n(json!(1.0)), "1");
    assert_eq!(n(json!(-0.0)), "0");
    assert_eq!(n(json!(1e6)), "1000000");
    assert_eq!(n(json!(0.000001)), "0.000001");
    assert_eq!(n(json!(1e-7)), "1e-7");
    assert_eq!(n(json!(1e21)), "1e+21");
}
