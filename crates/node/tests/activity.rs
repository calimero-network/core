//! What the account holder sees of what their passkeys did
//! (`GET /auth/activity`): each sign-in, with the app and origin its session
//! is bound to, and each change a passkey approved (a step-up), with its
//! method and subject, newest first. A refresh is no sign-in. Nobody else
//! sees it: another account reads only its own, and an API key is refused.

use std::time::Duration;

use calimero_identity::testing::SoftAuthenticator;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster, ORIGIN};

/// The listed entries, newest first.
fn entries(list: &Value) -> Vec<Value> {
    list["activity"].as_array().unwrap().clone()
}

/// A sign-up is a sign-in; later sign-ins and step-ups are listed newest
/// first, each with the passkey that made it; a refresh adds nothing; the
/// same entries are read through every node.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sign_ins_and_approvals_are_listed_with_their_passkey() {
    let c = Cluster::start(3).await;
    let (laptop, mut pk) = signed_up(&c, 0, "alice").await;
    let me = laptop.me().await.unwrap();
    let cred = me["passkeyId"].clone();
    let added = me["passkeys"][0]["addedAt"].as_u64().unwrap();

    let list = laptop.activity(None).await.unwrap();
    assert_eq!(
        entries(&list),
        vec![json!({
            "at": added, "passkey": cred, "kind": "signIn", "app": null, "origin": null,
            "method": null, "subject": null,
        })],
        "{list}"
    );

    // A step-up: the approval, by the passkey that made it.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (_id, key) = laptop
        .create_api_key(json!({ "label": "ci" }), &mut pk)
        .await
        .unwrap();
    let list = laptop.activity(None).await.unwrap();
    let newest = &entries(&list)[0];
    assert_eq!(newest["kind"], "approval", "{list}");
    assert_eq!(newest["method"], "__create_api_key", "{list}");
    assert_eq!(newest["passkey"], cred, "{list}");
    let approved = newest["at"].as_u64().unwrap();
    assert!(approved > added, "{list}");

    // An added passkey (approved by the first) signs in on another node.
    let mut phone_key = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = laptop.add_passkey(&mut phone_key, &mut pk).await.unwrap();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut phone_key).await.unwrap();

    // A refresh is no sign-in.
    let _s = laptop.refresh().await.unwrap();

    for i in 0..3 {
        let at = c.client(i);
        at.use_session(phone.session().await.unwrap()).await;
        let list = at.activity(None).await.unwrap();
        let got: Vec<(Value, Value, Value)> = entries(&list)
            .iter()
            .map(|e| (e["kind"].clone(), e["passkey"].clone(), e["method"].clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (json!("signIn"), json!(phone_cred), Value::Null),
                (json!("approval"), cred.clone(), json!("__add_passkey")),
                (json!("approval"), cred.clone(), json!("__create_api_key")),
                (json!("signIn"), cred.clone(), Value::Null),
            ],
            "node {i}: {list}"
        );
    }

    // `after`: only what came later.
    let later = laptop.activity(Some(approved)).await.unwrap();
    assert!(
        entries(&later)
            .iter()
            .all(|e| e["at"].as_u64().unwrap() > approved),
        "{later}"
    );
    assert!(entries(&later).len() < 4, "{later}");

    // The API key is the account's, but not its holder's session.
    let machine = c.client(2);
    machine.use_api_key(&key).await;
    let e = machine.activity(None).await.unwrap_err().to_string();
    assert!(e.starts_with("403"), "{e}");
}

/// Another account's activity is never listed: each account reads its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn each_account_reads_only_its_own_activity() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;
    let alices = alice.me().await.unwrap()["passkeyId"].clone();
    let bobs = bob.me().await.unwrap()["passkeyId"].clone();
    let _key = alice
        .create_api_key(json!({ "label": "ci" }), &mut alice_pk)
        .await
        .unwrap();

    let list = bob.activity(None).await.unwrap();
    assert_eq!(entries(&list).len(), 1, "{list}");
    assert!(
        entries(&list).iter().all(|e| e["passkey"] == bobs),
        "{list}"
    );
    let list = alice.activity(None).await.unwrap();
    assert_eq!(entries(&list).len(), 2, "{list}");
    assert!(
        entries(&list).iter().all(|e| e["passkey"] == alices),
        "{list}"
    );

    // Signed out, nobody reads any.
    let e = c.client(2).activity(None).await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
}
