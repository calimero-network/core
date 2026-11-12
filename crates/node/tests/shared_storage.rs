//! A write into a guarded field by an account outside the field's writer set
//! is refused inside the app, so it is never committed: the node runs a call
//! once, on the leader, with no peer to re-check it at merge.
//!
//! Builds the component `apps/kv-store-with-shared-storage` from this checkout
//! (cached under `$CARGO_TARGET_DIR/app-components`).

// Tracking: these tests call the route methods `calimero-client` wrote by hand,
// deprecated for its generated layer (`client.<group>().<method>()`); they move
// over, and the hand-written methods go, in a follow-up.
#![allow(
    deprecated,
    reason = "the hand-written route methods, until these tests use the generated ones"
)]

use calimero_client::{Client, Consistency};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster};

fn shared_kv_wasm() -> Vec<u8> {
    harness::fixture("apps/kv-store-with-shared-storage")
}

async fn map_get(cl: &Client, app: &str, key: &str) -> Value {
    cl.view(app, "mapGet", json!({ "key": key }), Consistency::Strong)
        .await
        .unwrap()
        .result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_non_writers_map_write_is_refused_and_a_writers_lands() {
    let c = Cluster::start(3).await;
    // Alice creates the app, so `init` makes her the map's only writer.
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bob_id = bob.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let bundle = alice.upload_bundle(shared_kv_wasm()).await.unwrap();
    let created = alice
        .post("apps", json!({ "name": "shared-kv", "bundle": bundle }))
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ok = alice
        .post(
            &format!("apps/{app}/access"),
            json!({ "access": { "kind": "accounts", "accounts": [bob_id] } }),
        )
        .await
        .unwrap();

    // Bob may call the app, but he is not in the map's writer set.
    let forged = bob
        .call(
            &app,
            "mapSet",
            json!({ "key": "evil", "value": "forged" }),
            None,
        )
        .await;
    let err = forged.expect_err("a non-writer's map write is refused");
    // An app error, not a trap: the method returned the app's refusal.
    assert_eq!(err.code(), Some("APP_ERROR"), "{err}");
    let details = err.details().cloned().unwrap_or_default();
    assert_eq!(
        details["code"], "NotWriter",
        "the refusal is the app's: {err}"
    );
    assert_eq!(details["fields"]["field"], "map", "{err}");

    // Read from another node than the one Bob called through: nothing landed.
    assert_eq!(map_get(&alice, &app, "evil").await, Value::Null);
    assert_eq!(map_get(&bob, &app, "evil").await, Value::Null);

    // The writer's write still lands, and every caller reads it.
    let _ok = alice
        .call(
            &app,
            "mapSet",
            json!({ "key": "theme", "value": "dark" }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(map_get(&bob, &app, "theme").await, json!("dark"));
    assert_eq!(map_get(&alice, &app, "theme").await, json!("dark"));
}
