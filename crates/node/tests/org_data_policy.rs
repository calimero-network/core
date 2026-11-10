//! An organization's data policy through the API, on a three-node cluster:
//! an admin proposes, a second admin approves, and every member reads the
//! adopted version; a member, a wrong hash and a policy that skips a version
//! change nothing.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

use calimero_client::Client;
use calimero_node::policy::DataPolicy;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster};

async fn account_of(client: &Client) -> String {
    client.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn policy(version: u64, prev: Option<&str>, signers: &[&str]) -> Value {
    json!({
        "version": version,
        "prev": prev,
        "readers": ["pkg:com.report"],
        "recipients": [format!("x25519:{}", "ab".repeat(32))],
        "sources": ["warehouse"],
        "sinks": [{ "name": "bucket", "from": ["nightly.render"] }],
        "signers": signers,
        "threshold": 2,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_policy_is_proposed_approved_by_a_second_admin_and_read_by_every_member() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bpk) = signed_up(&c, 1, "bob").await;
    let (carol, mut cpk) = signed_up(&c, 2, "carol").await;
    let (alice_id, bob_id, carol_id) = (
        account_of(&alice).await,
        account_of(&bob).await,
        account_of(&carol).await,
    );
    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (id, role) in [(&bob_id, "admin"), (&carol_id, "member")] {
        let _ = alice
            .post_confirmed(
                &format!("orgs/{org}/members"),
                json!({ "account": id, "role": role }),
                &mut apk,
            )
            .await
            .unwrap();
    }
    let path = format!("orgs/{org}/data-policy");

    let empty = carol.get(&path).await.unwrap();
    assert_eq!(empty["current"], Value::Null);
    assert_eq!(empty["pending"], Value::Null);

    let v1 = policy(1, None, &[&alice_id, &bob_id]);
    let hash = serde_json::from_value::<DataPolicy>(v1.clone())
        .unwrap()
        .hash()
        .unwrap();
    // A member is no admin; a version that skips a number is refused.
    let e = carol
        .post_confirmed(&path, v1.clone(), &mut cpk)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let e = alice
        .post_confirmed(&path, policy(2, None, &[&alice_id, &bob_id]), &mut apk)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("DATA_POLICY_CONFLICT"), "{e}");

    let out = alice.post_confirmed(&path, v1, &mut apk).await.unwrap();
    assert_eq!(out["adopted"], json!(false));
    let seen = carol.get(&path).await.unwrap();
    assert_eq!(seen["current"], Value::Null);
    assert_eq!(seen["pending"]["hash"], json!(hash));
    assert_eq!(seen["pending"]["proposer"], json!(alice_id));
    assert_eq!(seen["pending"]["approvals"], json!([alice_id]));

    // Another hash, a member, and the proposer again approve nothing.
    let approve = format!("{path}/approve");
    assert!(bob
        .post_confirmed(&approve, json!({ "hash": "00".repeat(32) }), &mut bpk)
        .await
        .is_err());
    assert!(carol
        .post_confirmed(&approve, json!({ "hash": hash }), &mut cpk)
        .await
        .is_err());
    assert!(alice
        .post_confirmed(&approve, json!({ "hash": hash }), &mut apk)
        .await
        .is_err());
    assert_eq!(carol.get(&path).await.unwrap()["current"], Value::Null);

    let out = bob
        .post_confirmed(&approve, json!({ "hash": hash }), &mut bpk)
        .await
        .unwrap();
    assert_eq!(out["adopted"], json!(true));
    for client in [&alice, &bob, &carol] {
        let got = client.get(&path).await.unwrap();
        assert_eq!(got["current"]["version"], json!(1));
        assert_eq!(got["currentHash"], json!(hash));
        assert_eq!(got["pending"], Value::Null);
    }

    // The next version chains to it, and a withdrawn proposal is gone.
    let v2 = policy(2, Some(&hash), &[&alice_id, &bob_id]);
    let _ = alice.post_confirmed(&path, v2, &mut apk).await.unwrap();
    assert_eq!(
        carol.get(&path).await.unwrap()["pending"]["policy"]["version"],
        json!(2)
    );
    let _ = bob
        .post_confirmed(&format!("{path}/withdraw"), json!({}), &mut bpk)
        .await
        .unwrap();
    let got = carol.get(&path).await.unwrap();
    assert_eq!(got["pending"], Value::Null);
    assert_eq!(got["current"]["version"], json!(1));

    // A person outside the organization reads nothing.
    let (dave, _dpk) = signed_up(&c, 0, "dave").await;
    assert!(dave.get(&path).await.is_err());
}
