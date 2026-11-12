//! Account proofs (`POST /auth/account-proof`): a node signs, with its attested
//! receipt key, that the passkey session asking is an account's and binds a
//! relying party's nonce and audience; the operator reads that key for any
//! node of the cluster (`GET /ops/nodes/{id}/receipt-key`) and checks the
//! proof itself. Only a passkey session gets one: no API key, no anonymous
//! caller; and only the operator reads the keys.

use calimero_client::attest::AccountProof;
use calimero_client::{Client, Error, Request};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, typed, wire, Cluster};
use reqwest::Method;

const NONCE: &str = "4f2b9c0d1e3a5b7c9d0e1f2a3b4c5d6e";

async fn ops(c: &Cluster, i: usize) -> Client {
    let ops = c.client(i);
    ops.use_api_key("ops-secret").await;
    ops
}

fn status(e: &Error) -> u16 {
    match e {
        Error::Api { status, .. } => *status,
        other => panic!("not an API refusal: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_nodes_account_proof_verifies_under_the_receipt_key_the_operator_reads() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let account = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();

    let answer = alice
        .auth()
        .account_proof_prove(NONCE, "mdma")
        .await
        .map(wire)
        .unwrap();
    let proof: AccountProof = serde_json::from_value(answer).unwrap();
    let s = &proof.statement;
    assert_eq!(s.account, account);
    assert_eq!(s.cluster, c.name);
    assert_eq!(s.nonce, NONCE);
    assert_eq!(s.audience, "mdma");
    assert_eq!(s.expires_at - s.issued_at, 300);

    // Signed by the node alice asked; the operator asks another one for its
    // key: what that node attested of its peer, over the operator's own
    // attested channel.
    assert_eq!(s.node, 1);
    let key = ops(&c, 2)
        .await
        .ops()
        .receipt_key(&s.node.to_string())
        .await
        .map(wire)
        .unwrap();
    assert_eq!(key["node"], json!(s.node));
    let key: [u8; 32] = hex::decode(key["key"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    assert!(
        proof.verify(&key),
        "the proof verifies under the attested key"
    );
    let mut claimed = proof.clone();
    claimed.statement.account = "someone-else".into();
    assert!(!claimed.verify(&key), "another account's name breaks it");
    let mut replayed = proof.clone();
    replayed.statement.nonce = "an-other-nonce-0123456789".into();
    assert!(!replayed.verify(&key), "another nonce breaks it");

    // Closed input: a nonce and an audience in plain text and within their
    // bounds, and nothing else in the body.
    for (body, code) in [
        (json!({ "nonce": "short", "audience": "mdma" }), "BAD_NONCE"),
        (
            json!({ "nonce": "x".repeat(129), "audience": "mdma" }),
            "BAD_NONCE",
        ),
        (
            json!({ "nonce": "a nonce with spaces!", "audience": "mdma" }),
            "BAD_NONCE",
        ),
        (json!({ "nonce": NONCE, "audience": "" }), "BAD_AUDIENCE"),
        (
            json!({ "nonce": NONCE, "audience": "a\"b" }),
            "BAD_AUDIENCE",
        ),
        (
            json!({ "nonce": NONCE, "audience": "m".repeat(129) }),
            "BAD_AUDIENCE",
        ),
    ] {
        let e = alice
            .transport()
            .json(Request::new(Method::POST, "auth/account-proof").json(body))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some(code), "{e}");
    }
    let refused = alice
        .transport()
        .json(
            Request::new(Method::POST, "auth/account-proof")
                .json(json!({ "nonce": NONCE, "audience": "mdma", "account": "someone-else" })),
        )
        .await
        .is_err();
    assert!(refused, "an account named in the body is refused");

    // Only a passkey session vouches for its account.
    let anonymous = c.client(0);
    let e = anonymous
        .auth()
        .account_proof_prove(NONCE, "mdma")
        .await
        .unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
    let (_id, api_key) = alice
        .auth()
        .apikey_create(typed(json!({ "label": "ci" })), Some(&mut alice_pk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let keyed = c.client(0);
    keyed.use_api_key(&api_key).await;
    let e = keyed
        .auth()
        .account_proof_prove(NONCE, "mdma")
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "an API key is not the person: {e}");

    // Only the operator reads receipt keys, and only of the cluster's nodes.
    let e = alice.ops().receipt_key("1").await.unwrap_err();
    assert!(matches!(status(&e), 401 | 403), "{e}");
    let e = ops(&c, 0).await.ops().receipt_key("99").await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    let own: Value = ops(&c, 0)
        .await
        .ops()
        .receipt_key("1")
        .await
        .map(wire)
        .unwrap();
    assert_eq!(own["node"], json!(1));
}
