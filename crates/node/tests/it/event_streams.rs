//! The share of a node's event and presence streams one account may hold
//! ([limits](https://calimero-network.github.io/core/protocol/limits/#node-limits)).

use crate::common;

use calimero_node::event_streams::MAX_EVENT_STREAMS_PER_ACCOUNT;
use common::{signed_up, typed, wire, Cluster};
use serde_json::json;

/// An account that holds its share of a node's streams is refused another `429`, with a
/// message naming the per-account limit, while another account on the same node and app
/// still opens one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_account_holding_its_stream_share_leaves_streams_for_others() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 0, "bob").await;
    let s = |v: serde_json::Value, k: &str| v[k].as_str().unwrap().to_owned();
    let org = s(
        alice
            .orgs()
            .create("Acme", None, None)
            .await
            .map(wire)
            .unwrap(),
        "orgId",
    );
    let token = s(
        alice
            .orgs()
            .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
            .await
            .map(wire)
            .unwrap(),
        "token",
    );
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bundle = alice
        .packages()
        .bundle_put(common::component("team-space"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "bundle": &bundle, "org": org }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();

    let mut held = Vec::new();
    for n in 0..MAX_EVENT_STREAMS_PER_ACCOUNT {
        let stream = alice.events(&app, 999_999_999).await;
        held.push(stream.unwrap_or_else(|e| panic!("stream {n} of the share: {e}")));
    }
    let Err(e) = alice.events(&app, 999_999_999).await else {
        panic!("a stream past the account's share was opened");
    };
    let status = match &e {
        calimero_client::Error::Api { status, .. } => *status,
        other => panic!("{other:?}"),
    };
    assert_eq!((status, e.code()), (429, Some("RATE_LIMITED")), "{e}");
    assert!(e.to_string().contains("per-account limit"), "{e}");

    let _bob_events = bob
        .events(&app, 0)
        .await
        .expect("another account still opens a stream on the node");
    let _bob_presence = bob
        .presence_stream(&app)
        .await
        .expect("another account still opens a presence stream on the node");
    drop(held);
}
