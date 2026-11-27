//! Refreshes on the production `api::REFRESH_ROTATE_AFTER`: a refresh token
//! younger than it mints a session from the system state at a read index and
//! writes nothing to the system log; an older one is exchanged in the log as
//! every refresh was before. What the log decides still holds at once on
//! every node: a sign-out, a sign-out everywhere, a passkey removal and a
//! recovery end the refresh token whatever node it is presented to, and a
//! token presented again after it was exchanged revokes its session.
//! (`tests/it/cluster.rs` runs the harness's zero: every refresh exchanges.)

use std::time::Duration;

use calimero_client::{Client, Passkey, Request, Session};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::api::REFRESH_ROTATE_AFTER;
use calimero_node::types::RECOVERY_DELAY_SECS;
use reqwest::Method;
use serde_json::json;

use crate::harness;

use harness::{signed_up, wire, Cluster, ORIGIN};

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
            assert_eq!(
                other.auth().me().await.map(wire).unwrap()["accountId"],
                json!(s.account_id)
            );
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
    let _ok = laptop.auth().logout().await.unwrap();
    for i in 0..3 {
        let holder = holding(&c, i, kept.clone()).await;
        assert!(refused(&holder).await, "signed out, node {i}");
    }
    let still = other.refresh().await.unwrap();
    assert!(other.auth().me().await.is_ok());

    // Sign-out everywhere, made on node 3: every session of the account ends.
    let third = c.client(2);
    let _s = third.sign_in(&mut pk).await.unwrap();
    let thirds = third.refresh().await.unwrap();
    let _ok = third.auth().logout_all().await.unwrap();
    for i in 0..3 {
        for session in [&still, &thirds] {
            let holder = holding(&c, i, session.clone()).await;
            assert!(refused(&holder).await, "signed out everywhere, node {i}");
            let e = holder.auth().me().await.unwrap_err();
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
        .auth()
        .passkey_remove(&phone_cred, Some(&mut pk))
        .await
        .unwrap();
    for i in 0..3 {
        let holder = holding(&c, i, phones.clone()).await;
        assert!(refused(&holder).await, "passkey removed, node {i}");
    }
    let _s = device.refresh().await.unwrap();
    assert!(
        device.auth().me().await.is_ok(),
        "the other passkey's session stays"
    );
}

/// An account recovery ends the lost device's kept refresh token on every node.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_recovery_ends_a_kept_refresh_token() {
    let c = Cluster::start_with(3, |cfg| cfg.refresh_rotate_after = REFRESH_ROTATE_AFTER).await;
    let (alice, mut lost) = signed_up(&c, 0, "alice").await;
    let account = alice.auth().me().await.map(wire).unwrap()["accountId"].clone();
    let code = alice
        .auth()
        .recovery_create(Some(&mut lost))
        .await
        .map(wire)
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    let kept = alice.refresh().await.unwrap();
    assert!(held_unexchanged(&c, &kept.refresh_token).await);

    let rescuer = c.client(1);
    let mut laptop = SoftAuthenticator::new("localhost", ORIGIN);
    let begin = rescuer
        .auth()
        .account_recovery_begin(&code)
        .await
        .map(wire)
        .unwrap();
    let registration = laptop.create(&begin["options"]);
    let _started = rescuer
        .auth()
        .account_recovery_start(begin["challengeId"].as_str().unwrap(), &code, registration)
        .await
        .unwrap();
    let ms = i64::try_from(RECOVERY_DELAY_SECS * 1000).unwrap();
    for r in c.nodes.iter().flatten() {
        r.node.advance_clock(ms);
    }
    let _finished = c
        .client(2)
        .auth()
        .account_recovery_finish(Some(account.as_str().unwrap()), Some(&mut laptop))
        .await
        .map(wire)
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
    let e = owner.auth().me().await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
    assert!(refused(&owner).await, "the new token went with the session");
}

/// A device key for the holder tests: a P-256 key as a browser's non-extractable
/// WebCrypto key is, its raw public point as the session names it.
struct Holder {
    pair: ring::signature::EcdsaKeyPair,
    public: String,
}

impl Holder {
    fn new() -> Self {
        use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
        let rng = ring::rand::SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let public = calimero_identity::b64url(pair.public_key().as_ref());
        Self { pair, public }
    }

    /// Its proof for `token`, made at `at`.
    fn proof(&self, token: &str, at: u64) -> serde_json::Value {
        let message = calimero_node::proof::refresh_holder_message(&refresh_hash(token), at);
        let rng = ring::rand::SystemRandom::new();
        let signature = self.pair.sign(&rng, &message).unwrap();
        json!({ "at": at, "signature": calimero_identity::b64url(signature.as_ref()) })
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// `client` signs in with `pk`, binding the session to `holder`'s key.
async fn sign_in_held(client: &Client, pk: &mut SoftAuthenticator, holder: &Holder) -> Session {
    let begin = client
        .transport()
        .json(
            Request::new(Method::POST, "auth/passkey/login/begin")
                .json(json!({ "holderKey": holder.public })),
        )
        .await
        .unwrap();
    let challenge =
        calimero_identity::from_b64url(begin["options"]["challenge"].as_str().unwrap()).unwrap();
    let assertion = pk.get(&challenge);
    let id = begin["challengeId"].as_str().unwrap();
    client.auth().login_finish(id, assertion).await.unwrap()
}

/// `POST /auth/refresh` on node `i` with `body`, signed out: the answer, or the refusal's text.
async fn refresh_raw(
    c: &Cluster,
    i: usize,
    body: serde_json::Value,
) -> Result<serde_json::Value, String> {
    c.client(i)
        .transport()
        .json(Request::new(Method::POST, "auth/refresh").json(body))
        .await
        .map_err(|e| e.to_string())
}

/// A session bound to a holder key refreshes only with that key's proof, on any node: the
/// token alone, copied off the device, is refused, as is a proof by another key, a stale
/// one, or one for another token. None of those spends the token or ends the session, and
/// the token lives 7 days rather than 30.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_holder_bound_refresh_token_refreshes_only_with_its_holders_proof() {
    let c = Cluster::start_with(3, |cfg| cfg.refresh_rotate_after = REFRESH_ROTATE_AFTER).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let device = Holder::new();
    let s = sign_in_held(&alice, &mut pk, &device).await;
    let token = s.refresh_token.clone();

    let hash = refresh_hash(&token);
    let record = alice_record(&c, &hash).await;
    assert_eq!(record.holder.as_deref(), Some(device.public.as_str()));
    let lifetime = record.expires_at - now();
    assert!(
        lifetime <= 7 * 24 * 3600 && lifetime > 7 * 24 * 3600 - 120,
        "a holder-bound token lives 7 days: {lifetime}"
    );

    let stolen = json!({ "refreshToken": token });
    for i in 0..3 {
        let e = refresh_raw(&c, i, stolen.clone()).await.unwrap_err();
        assert!(e.starts_with("401 HOLDER_PROOF_REQUIRED"), "{e}");
    }
    let thief = Holder::new();
    let e = refresh_raw(
        &c,
        1,
        json!({ "refreshToken": token, "holderProof": thief.proof(&token, now()) }),
    )
    .await
    .unwrap_err();
    assert!(e.starts_with("401 BAD_HOLDER_PROOF"), "another key: {e}");
    let e = refresh_raw(
        &c,
        1,
        json!({ "refreshToken": token, "holderProof": device.proof(&token, now() - 600) }),
    )
    .await
    .unwrap_err();
    assert!(e.starts_with("401 BAD_HOLDER_PROOF"), "stale: {e}");
    let e = refresh_raw(
        &c,
        1,
        json!({ "refreshToken": token, "holderProof": device.proof("rt_other", now()) }),
    )
    .await
    .unwrap_err();
    assert!(
        e.starts_with("401 BAD_HOLDER_PROOF"),
        "another token's: {e}"
    );

    for i in 0..3 {
        let fresh = refresh_raw(
            &c,
            i,
            json!({ "refreshToken": token, "holderProof": device.proof(&token, now()) }),
        )
        .await
        .unwrap();
        assert_eq!(
            fresh["refreshToken"],
            json!(token),
            "kept while young, node {i}"
        );
        let on = holding(
            &c,
            i,
            Session {
                session_token: fresh["sessionToken"].as_str().unwrap().to_owned(),
                ..s.clone()
            },
        )
        .await;
        assert_eq!(
            on.auth().me().await.map(wire).unwrap()["accountId"],
            json!(s.account_id)
        );
    }
    assert!(
        held_unexchanged(&c, &token).await,
        "no refusal spent the token"
    );
}

/// An exchanged holder-bound token's successor is bound to the same key, and keeps
/// the 7-day lifetime.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_exchanged_holder_bound_token_stays_bound() {
    let rotate = Duration::from_secs(2);
    let c = Cluster::start_with(3, |cfg| cfg.refresh_rotate_after = rotate).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let device = Holder::new();
    let first = sign_in_held(&alice, &mut pk, &device).await.refresh_token;
    tokio::time::sleep(rotate + Duration::from_millis(1100)).await;

    let next = refresh_raw(
        &c,
        2,
        json!({ "refreshToken": first, "holderProof": device.proof(&first, now()) }),
    )
    .await
    .unwrap();
    let next = next["refreshToken"].as_str().unwrap().to_owned();
    assert_ne!(next, first, "exchanged once old enough");
    let record = alice_record(&c, &refresh_hash(&next)).await;
    assert_eq!(record.holder.as_deref(), Some(device.public.as_str()));
    assert!(record.expires_at - now() <= 7 * 24 * 3600);
    let e = refresh_raw(&c, 0, json!({ "refreshToken": next }))
        .await
        .unwrap_err();
    assert!(e.starts_with("401 HOLDER_PROOF_REQUIRED"), "{e}");
}

/// A holder key that is not an uncompressed P-256 point is refused before the ceremony.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_malformed_holder_key_is_refused_at_sign_in() {
    let c = Cluster::start(1).await;
    let e = c
        .client(0)
        .transport()
        .json(
            Request::new(Method::POST, "auth/passkey/login/begin")
                .json(json!({ "holderKey": calimero_identity::b64url(&[2; 33]) })),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("400 BAD_HOLDER_KEY"), "{e}");
}

/// The refresh record every node holds under `hash`, once one holds it.
async fn alice_record(c: &Cluster, hash: &str) -> calimero_node::types::RefreshRecord {
    for _ in 0..200 {
        if let Some(r) = c.nodes[0]
            .as_ref()
            .and_then(|r| r.node.sys(|s| s.refresh.get(hash).cloned()))
        {
            return r;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no node holds the refresh record");
}
