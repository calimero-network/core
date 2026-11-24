//! A write into a guarded field by an account outside the field's writer set
//! is refused inside the app, so it is never committed: the node runs a call
//! once, on the leader, with no peer to re-check it at merge.
//!
//! Builds the component `apps/team-space` from this checkout
//! (cached under `$CARGO_TARGET_DIR/app-components`).

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use calimero_client::types::ViewsRequestConsistency;
use calimero_client::Client;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, typed, wire, Cluster};

fn team_space_wasm() -> Vec<u8> {
    harness::fixture("apps/team-space")
}

async fn map_get(cl: &Client, app: &str, key: &str) -> Value {
    cl.apps()
        .view(
            app,
            "mapGet",
            Some(json!({ "key": key })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_non_writers_map_write_is_refused_and_a_writers_lands() {
    let c = Cluster::start(3).await;
    // Alice creates the app, so `init` makes her the map's only writer.
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Bob is a member of Alice's organization: an `accounts` list reaches its members.
    let org = alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bundle = alice
        .packages()
        .bundle_put(team_space_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let created = alice
        .apps()
        .create(typed(
            json!({ "name": "team-space", "bundle": bundle, "org": org }),
        ))
        .await
        .map(wire)
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let _ok = alice
        .apps()
        .access(
            &app,
            vec![typed(json!({ "kind": "account", "account": bob_id }))],
        )
        .await
        .unwrap();

    // Bob may call the app, but he is not in the map's writer set.
    let forged = bob
        .apps()
        .call(
            &app,
            "mapSet",
            Some(json!({ "key": "evil", "value": "forged" })),
            None,
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
        .apps()
        .call(
            &app,
            "mapSet",
            Some(json!({ "key": "theme", "value": "dark" })),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(map_get(&bob, &app, "theme").await, json!("dark"));
    assert_eq!(map_get(&alice, &app, "theme").await, json!("dark"));
}
