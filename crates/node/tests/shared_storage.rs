//! A write into a `SharedStorage` cell by an account outside the cell's writer
//! set is refused inside the SDK, so it is never committed: the node runs a
//! call once, on the leader, with no peer to re-check it at merge.
//!
//! Needs `apps/kv-store-with-shared-storage/res/kv_store_with_shared_storage.wasm`
//! built from THIS checkout's SDK (a fixture built from a published SDK proves
//! nothing): `cargo mero build --manifest-path apps/kv-store-with-shared-storage/Cargo.toml`.
//! Set `KV_SHARED_WASM` to use another path.

use std::path::PathBuf;

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
    let path = std::env::var("KV_SHARED_WASM")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
                "../../apps/kv-store-with-shared-storage/res/kv_store_with_shared_storage.wasm",
            )
        });
    std::fs::read(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e}; build it with cargo mero build --manifest-path \
             apps/kv-store-with-shared-storage/Cargo.toml",
            path.display()
        )
    })
}

async fn map_get(cl: &Client, app: &str, key: &str) -> Value {
    cl.view(app, "map_get", json!({ "key": key }), Consistency::Strong)
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
            "map_set",
            json!({ "key": "evil", "value": "forged" }),
            None,
        )
        .await;
    let err = forged.expect_err("a non-writer's map write is refused");
    // An app error, not a trap: the method returned the SDK's refusal.
    assert_eq!(err.code(), Some("APP_ERROR"), "{err}");
    let why = err
        .details()
        .map(|d| d["error"].to_string())
        .unwrap_or_default();
    assert!(
        why.contains("may not write this cell"),
        "the refusal is the SDK's: {why}"
    );

    // Read from another node than the one Bob called through: nothing landed.
    assert_eq!(map_get(&alice, &app, "evil").await, Value::Null);
    assert_eq!(map_get(&bob, &app, "evil").await, Value::Null);

    // The writer's write still lands, and every caller reads it.
    let _ok = alice
        .call(
            &app,
            "map_set",
            json!({ "key": "theme", "value": "dark" }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(map_get(&bob, &app, "theme").await, json!("dark"));
    assert_eq!(map_get(&alice, &app, "theme").await, json!("dark"));
}
