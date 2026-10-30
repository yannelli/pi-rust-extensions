use super::*;

fn values(prefix: &str) -> Option<Vec<String>> {
    complete(prefix).map(|items| items.into_iter().map(|i| i.value).collect())
}

#[test]
fn completes_actions_and_backends() {
    assert_eq!(values("lo").unwrap(), ["login", "logout"]);
    assert_eq!(values("login ").unwrap().len(), NAMES.len());
    assert_eq!(values("logout ").unwrap()[0], "logout typesafe");
    assert_eq!(values("login li").unwrap(), ["login liquid"]);
    assert_eq!(values("login x"), None);
    assert_eq!(values("status "), None);
    assert_eq!(values("zz"), None);
    assert_eq!(values("").unwrap().len(), ACTIONS.len());
}
