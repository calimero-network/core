//! Three members' writes to one set (v1 ran these on three replicas and merged them as an
//! add-wins union; here one log orders them, so there is nothing to merge).

#![cfg(not(target_arch = "wasm32"))]

mod common;

use calimero_app_sdk::testing::Org;
use serde_json::json;

#[test]
fn distinct_elements_union() {
    let mut t = common::app(Org::new().members(["alice", "bob", "carol"]));
    for who in ["alice", "bob", "carol"] {
        let added = t.as_(who).call("add", json!({ "value": who })).unwrap();
        assert_eq!(added, json!(true));
    }
    assert_eq!(t.as_("owner").view("len", json!({})).unwrap(), json!("3"));
    assert_eq!(
        t.as_("alice")
            .view("contains", json!({ "value": "bob" }))
            .unwrap(),
        json!(true)
    );
    assert_eq!(
        t.as_("owner").view("elements", json!({})).unwrap(),
        json!(["alice", "bob", "carol"])
    );
}

#[test]
fn add_and_remove_emit_only_on_change() {
    let mut t = common::app(Org::new());
    let o = |t: &mut calimero_app_sdk::testing::TestApp, m: &str, v: &str| {
        t.as_("owner").call(m, json!({ "value": v })).unwrap()
    };
    assert_eq!(o(&mut t, "add", "x"), json!(true));
    assert_eq!(o(&mut t, "add", "x"), json!(false));
    assert_eq!(o(&mut t, "remove", "x"), json!(true));
    assert_eq!(o(&mut t, "remove", "x"), json!(false));
    t.as_("owner").call("clear", json!({})).unwrap();
    let names: Vec<String> = t.events_for("owner").into_iter().map(|(n, _)| n).collect();
    assert_eq!(names, ["Added", "Removed"]);
}
