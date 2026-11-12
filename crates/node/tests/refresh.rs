//! Refreshes on the production `api::REFRESH_ROTATE_AFTER`: a refresh token
//! younger than it mints a session from the system state at a read index and
//! writes nothing to the system log; an older one is exchanged in the log as
//! every refresh was before. What the log decides still holds at once on
//! every node: a sign-out, a sign-out everywhere, a passkey removal and a
//! recovery end the refresh token whatever node it is presented to, and a
//! token presented again after it was exchanged revokes its session.
//! (`tests/cluster.rs` runs the harness's zero: every refresh exchanges.)

// Tracking: these tests call the route methods `calimero-client` wrote by hand,
// deprecated for its generated layer (`client.<group>().<method>()`); they move
// over, and the hand-written methods go, in a follow-up.
#![allow(
    deprecated,
    reason = "the hand-written route methods, until these tests use the generated ones"
)]

use std::time::Duration;

use calimero_client::{Client, Passkey, Session};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::api::REFRESH_ROTATE_AFTER;
use calimero_node::types::RECOVERY_DELAY_SECS;
use serde_json::json;

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster, ORIGIN};

/// The hash the log keeps a refresh token under.
fn refresh_hash(token: &str) -> String {
    calimero_identity::b64url(&calimero_identity::sha256(token.as_bytes()))
}

/// Whether every node comes to hold `token` as live and never exchanged: no
/// `UseRefresh` applied for it anywhere. A replica applies a write the leader
/// committed a beat later, so each node is waited for.
async fn held_unexchanged(c: &Cluster, token: &str) -> bool {
    let hash = refresh_hash(token);
    let held = || {
        c.nodes.iter().flatten().all(|r| {
            r.node
                .sys(|s| s.refresh.contains_key(&hash) && !s.rotated_refresh.contains_key(&hash))
        })
    };
    for _ in 0..200 {
        if held() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// A client on node `i` holding `session`.
async fn holding(c: &Cluster, i: usize, session: Session) -> Client {
    let client = c.client(i);
    client.use_session(session).await;
    client
}

/// Whether `client`'s refresh is refused `401` (its token no longer opens a
/// session).
async fn refused(client: &Client) -> bool {
    match client.refresh().await {
        Ok(_) => false,
        Err(e) => e.to_string().starts_with("401"),
    }
}

/// A young refresh token refreshes on every node, keeps its value and is never
/// exchanged in the log, and the sessions it mints verify everywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_young_refresh_token_refreshes_on_every_node_without_a_system_write() {
    let c = Cluster::start_with(3, |cfg| cfg.refresh_rotate_after = REFRESH_ROTATE_AFTER).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let first = alice.session().await.unwrap();
    for round in 0..6 {
        let on = holding(&c, round % 3, alice.session().await.unwrap()).await;
        let s = on.refresh().await.unwrap();
        assert_eq!(s.refresh_token, first.refresh_token, "round {round}");
        for i in 0..3 {
            let other = holding(&c, i, s.clone()).await;
            assert_eq!(other.me().await.unwrap()["accountId"], json!(s.account_id));
        }
    }
    assert!(held_unexchanged(&c, &first.refresh_token).await);
}

/// A sign-out, a sign-out everywhere and a passkey removal, each made through
/// one node, end a kept refresh token on every node at once.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn revocations_end_a_kept_refresh_token_on_every_node() {
    let c = Cluster::start_with(3, |cfg| cfg.refresh_rotate_after = REFRESH_ROTATE_AFTER).await;
    let (laptop, mut pk) = signed_up(&c, 0, "alice").await;

    // Sign-out: this session's token ends; another of the account's stays.
    let other = c.client(1);
    let _s = other.sign_in(&mut pk).await.unwrap();
    let kept = laptop.refresh().await.unwrap();
    assert!(held_unexchanged(&c, &kept.refresh_token).await);
    let _ok = laptop.post("auth/logout", json!({})).await.unwrap();
    for i in 0..3 {
        let holder = holding(&c, i, kept.clone()).await;
        assert!(refused(&holder).await, "signed out, node {i}");
    }
    let still = other.refresh().await.unwrap();
    assert!(other.me().await.is_ok());

    // Sign-out everywhere, made on node 3: every session of the account ends.
    let third = c.client(2);
    let _s = third.sign_in(&mut pk).await.unwrap();
    let thirds = third.refresh().await.unwrap();
    let _ok = third.sign_out_everywhere().await.unwrap();
    for i in 0..3 {
        for session in [&still, &thirds] {
            let holder = holding(&c, i, session.clone()).await;
            assert!(refused(&holder).await, "signed out everywhere, node {i}");
            let e = holder.me().await.unwrap_err();
            assert!(e.to_string().starts_with("401"), "node {i}: {e}");
        }
    }

    // A passkey removed through one device ends the sessions it opened.
    let device = c.client(0);
    let _s = device.sign_in(&mut pk).await.unwrap();
    let mut phone_key = SoftAuthenticator::new("localhost", ORIGIN);
    let phone_cred = device.add_passkey(&mut phone_key, &mut pk).await.unwrap();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut phone_key).await.unwrap();
    let phones = phone.refresh().await.unwrap();
    assert!(held_unexchanged(&c, &phones.refresh_token).await);
    let _ok = device
        .delete_confirmed(&format!("auth/passkeys/{phone_cred}"), &mut pk)
        .await
        .unwrap();
    for i in 0..3 {
        let holder = holding(&c, i, phones.clone()).await;
        assert!(refused(&holder).await, "passkey removed, node {i}");
    }
    let _s = device.refresh().await.unwrap();
    assert!(
        device.me().await.is_ok(),
        "the other passkey's session stays"
    );
}

/// An account recovery ends the lost device's kept refresh token on every node.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_recovery_ends_a_kept_refresh_token() {
    let c = Cluster::start_with(3, |cfg| cfg.refresh_rotate_after = REFRESH_ROTATE_AFTER).await;
    let (alice, mut lost) = signed_up(&c, 0, "alice").await;
    let account = alice.me().await.unwrap()["accountId"].clone();
    let code = alice
        .post_confirmed("auth/recovery", json!({}), &mut lost)
        .await
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let kept = alice.refresh().await.unwrap();
    assert!(held_unexchanged(&c, &kept.refresh_token).await);

    let rescuer = c.client(1);
    let mut laptop = SoftAuthenticator::new("localhost", ORIGIN);
    let begin = rescuer
        .post("auth/recovery/begin", json!({ "code": code }))
        .await
        .unwrap();
    let registration = laptop.create(&begin["options"]);
    let _started = rescuer
        .post(
            "auth/recovery/start",
            json!({ "challengeId": begin["challengeId"], "code": code, "registration": registration }),
        )
        .await
        .unwrap();
    let ms = i64::try_from(RECOVERY_DELAY_SECS * 1000).unwrap();
    for r in c.nodes.iter().flatten() {
        r.node.advance_clock(ms);
    }
    let _finished = c
        .client(2)
        .post_confirmed(
            "auth/recovery/finish",
            json!({ "accountId": account }),
            &mut laptop,
        )
        .await
        .unwrap();
    for i in 0..3 {
        let holder = holding(&c, i, kept.clone()).await;
        assert!(refused(&holder).await, "recovered, node {i}");
    }
}

/// A token past the rotation age is exchanged in the log; the old one,
/// presented again on another node, is a stolen copy and ends the session.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_old_refresh_token_is_exchanged_and_its_reuse_revokes_the_session() {
    let rotate = Duration::from_secs(2);
    let c = Cluster::start_with(3, |cfg| cfg.refresh_rotate_after = rotate).await;
    let (owner, _pk) = signed_up(&c, 0, "owner").await;
    let young = owner.refresh().await.unwrap();
    let first = young.refresh_token.clone();
    assert!(held_unexchanged(&c, &first).await);

    tokio::time::sleep(rotate + Duration::from_millis(1100)).await;
    let rotated = owner.refresh().await.unwrap();
    assert_ne!(rotated.refresh_token, first, "exchanged once old enough");
    let thief = holding(&c, 1, young).await;
    let e = thief.refresh().await.unwrap_err().to_string();
    assert!(e.contains("already used"), "{e}");
    let e = owner.me().await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
    assert!(refused(&owner).await, "the new token went with the session");
}
