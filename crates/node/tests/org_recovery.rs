//! Organization recovery (`docs/specs/org-recovery.md`) through the API, on a
//! three-node cluster: an owner who lost every passkey gets the organization
//! back with its recovery seed, after the delay; a wrong seed, a non-owner, a
//! rotated seed and a node writing to the log on its own recover nothing.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

// Tracking: these tests call the route methods `calimero-client` wrote by hand,
// deprecated for its generated layer (`client.<group>().<method>()`); they move
// over, and the hand-written methods go, in a follow-up.
#![allow(
    deprecated,
    reason = "the hand-written route methods, until these tests use the generated ones"
)]

use calimero_client::Client;
use calimero_identity::recovery::{recovery_public, sign_recovery};
use calimero_node::system::SystemCmd;
use calimero_node::types::RECOVERY_DELAY_SECS;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, Cluster};

async fn account_of(client: &Client) -> String {
    client.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Moves every node's clock by `secs`.
fn advance(c: &Cluster, secs: u64) {
    let ms = i64::try_from(secs * 1000).unwrap();
    for r in c.nodes.iter().flatten() {
        r.node.advance_clock(ms);
    }
}

fn code(e: &calimero_client::Error) -> Option<&str> {
    e.code()
}

/// Alice creates an organization with a recovery key, then loses every
/// passkey. Signed up again with a new passkey, she starts a recovery with
/// the seed; every member sees it; it finishes only after the delay, and
/// only for the account the seed signed for. Then the new account owns the
/// organization and its apps, and the old one reaches neither.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_owner_who_lost_every_passkey_recovers_the_org() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let (bob, _bpk) = signed_up(&c, 1, "bob").await;
    let bob_id = account_of(&bob).await;
    let seed = calimero_identity::recovery::new_recovery_seed();
    let org = alice
        .post(
            "orgs",
            json!({ "name": "Acme", "recoveryKey": recovery_public(&seed) }),
        )
        .await
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .post_confirmed(
            &format!("orgs/{org}/members"),
            json!({ "account": bob_id, "role": "member" }),
            &mut apk,
        )
        .await
        .unwrap();
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice
        .post(
            "apps",
            json!({ "name": "kv", "bundle": bundle, "org": org }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let o = bob.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["recovery"]["publicKey"], json!(recovery_public(&seed)));
    assert_eq!(o["recovery"]["pending"], Value::Null);

    // Every passkey lost: alice signs up again, with a new passkey.
    drop(apk);
    let (again, mut again_pk) = signed_up(&c, 2, "alice-again").await;
    let again_id = account_of(&again).await;
    let started = again.start_recovery(&org, &again_id, &seed).await.unwrap();
    let ready = started["readyAt"].as_u64().unwrap();
    assert_eq!(
        ready - started["startedAt"].as_u64().unwrap(),
        RECOVERY_DELAY_SECS
    );
    // Every member sees it.
    let o = bob.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["recovery"]["pending"]["account"], json!(again_id));
    assert_eq!(o["recovery"]["pending"]["readyAt"], json!(ready));

    // Not before the delay; not for anyone else.
    let e = again
        .finish_recovery(&org, &mut again_pk)
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("RECOVERY_NOT_READY"), "{e}");
    advance(&c, RECOVERY_DELAY_SECS);
    let (carol, mut cpk) = signed_up(&c, 0, "carol").await;
    let e = carol.finish_recovery(&org, &mut cpk).await.unwrap_err();
    assert_eq!(code(&e), Some("NOT_FOUND"), "{e}");

    let done = again.finish_recovery(&org, &mut again_pk).await.unwrap();
    assert_eq!(done["previousOwners"], json!([account_of(&alice).await]));
    let o = again.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["members"][&again_id], json!("owner"));
    assert_eq!(o["members"][&bob_id], json!("member"));
    assert!(o["members"].get(account_of(&alice).await).is_none());
    assert_eq!(o["recovery"]["pending"], Value::Null);
    // The app moved with the organization; the old account reaches neither.
    let a = again.get(&format!("apps/{app}")).await.unwrap();
    assert_eq!(a["owner"], json!(again_id));
    assert!(alice.get(&format!("orgs/{org}")).await.is_err());
    assert!(alice.get(&format!("apps/{app}")).await.is_err());
    // The new owner acts for the organization with the new passkey.
    let _ = again
        .post_confirmed(
            &format!("orgs/{org}/members"),
            json!({ "account": bob_id, "role": "admin" }),
            &mut again_pk,
        )
        .await
        .unwrap();
}

/// A wrong seed, a member who is not the owner, a rotated seed and a replayed
/// signature recover nothing; neither does a node that writes a recovery to
/// the system log without the seed or the recovering account's passkey.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_wrong_seed_a_non_owner_a_rotated_seed_and_a_node_recover_nothing() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bpk) = signed_up(&c, 1, "bob").await;
    let bob_id = account_of(&bob).await;
    // Created without a key, as every organization before this change.
    let org = alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = alice
        .post_confirmed(
            &format!("orgs/{org}/members"),
            json!({ "account": bob_id, "role": "admin" }),
            &mut apk,
        )
        .await
        .unwrap();
    assert!(alice
        .get(&format!("orgs/{org}"))
        .await
        .unwrap()
        .get("recovery")
        .is_none());
    let seed = [5_u8; 32];
    let e = bob.start_recovery(&org, &bob_id, &seed).await.unwrap_err();
    assert_eq!(code(&e), Some("WRONG_RECOVERY_KEY"), "{e}");

    // Only the owner sets it, with a passkey: an admin cannot.
    let e = bob
        .set_recovery_key(&org, Some(&recovery_public(&seed)), &mut bpk)
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("FORBIDDEN"), "{e}");
    let e = alice
        .post(
            &format!("orgs/{org}/recovery"),
            json!({ "publicKey": recovery_public(&seed) }),
        )
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("CONFIRM_REQUIRED"), "{e}");
    let _ = alice
        .set_recovery_key(&org, Some(&recovery_public(&seed)), &mut apk)
        .await
        .unwrap();

    // A wrong seed is refused; the right one starts, once at a time.
    let e = bob
        .start_recovery(&org, &bob_id, &[6; 32])
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("WRONG_RECOVERY_KEY"), "{e}");
    let nonce = "n-1";
    let signed = json!({ "nonce": nonce, "signature": sign_recovery(&seed, &org, &bob_id, nonce) });
    let start = format!("orgs/{org}/recovery/start");
    let _ = bob.post(&start, signed.clone()).await.unwrap();
    let e = bob.start_recovery(&org, &bob_id, &seed).await.unwrap_err();
    assert_eq!(code(&e), Some("RECOVERY_PENDING"), "{e}");

    // Only the owner cancels; the cancelled signature does not start again.
    assert!(bob.cancel_recovery(&org, &mut bpk).await.is_err());
    let _ = alice.cancel_recovery(&org, &mut apk).await.unwrap();
    let e = bob.post(&start, signed).await.unwrap_err();
    assert_eq!(code(&e), Some("REJECTED"), "{e}");

    // A node logging a start the seed signed for someone else, or a finish
    // without the account's passkey, is refused where it applies.
    let node = &c.nodes[0].as_ref().unwrap().node;
    let redirected = SystemCmd::StartRecovery {
        org: org.clone(),
        account: account_of(&alice).await,
        nonce: "n-2".into(),
        signature: sign_recovery(&seed, &org, &bob_id, "n-2"),
        at: 0,
    };
    assert!(node.sys_write(&redirected).await.is_err());
    let _ = bob.start_recovery(&org, &bob_id, &seed).await.unwrap();
    advance(&c, RECOVERY_DELAY_SECS);
    let forged_finish = SystemCmd::FinishRecovery {
        org: org.clone(),
        at: u64::MAX,
        actor: bob_id.clone(),
        consent: Value::Null,
    };
    assert!(node.sys_write(&forged_finish).await.is_err());

    // Rotation cancels the recovery under way and retires the old seed.
    let new_seed = [7_u8; 32];
    let _ = alice
        .set_recovery_key(&org, Some(&recovery_public(&new_seed)), &mut apk)
        .await
        .unwrap();
    let e = bob.finish_recovery(&org, &mut bpk).await.unwrap_err();
    assert_eq!(code(&e), Some("NOT_FOUND"), "{e}");
    let e = bob.start_recovery(&org, &bob_id, &seed).await.unwrap_err();
    assert_eq!(code(&e), Some("WRONG_RECOVERY_KEY"), "{e}");
    let o = alice.get(&format!("orgs/{org}")).await.unwrap();
    assert_eq!(o["members"][account_of(&alice).await], json!("owner"));
    assert_eq!(
        o["recovery"]["publicKey"],
        json!(recovery_public(&new_seed))
    );

    // Removing the key: nothing to recover with.
    let _ = alice.set_recovery_key(&org, None, &mut apk).await.unwrap();
    let e = bob
        .start_recovery(&org, &bob_id, &new_seed)
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("WRONG_RECOVERY_KEY"), "{e}");
}
