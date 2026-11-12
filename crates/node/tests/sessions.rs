//! What the account holder sees of their own passkeys and sessions: when each
//! passkey was added and last signed in (`/auth/me`), and the account's live
//! sessions, listed and ended one at a time (`/auth/sessions`). Nobody else
//! sees or ends them: another account's session is `404`, and an API key is
//! refused the list.

// Tracking: these tests call the route methods `calimero-client` wrote by hand,
// deprecated for its generated layer (`client.<group>().<method>()`); they move
// over, and the hand-written methods go, in a follow-up.
#![allow(
    deprecated,
    reason = "the hand-written route methods, until these tests use the generated ones"
)]

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

/// The passkey `id` as `/auth/me` lists it.
fn passkey<'a>(me: &'a Value, id: &str) -> &'a Value {
    me["passkeys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == json!(id))
        .unwrap_or_else(|| panic!("no passkey {id} in {me}"))
}

/// The listed sessions.
fn sessions(list: &Value) -> &Vec<Value> {
    list["sessions"].as_array().unwrap()
}

/// The caller's own session, as the list marks it.
fn current(list: &Value) -> Value {
    let mine: Vec<&Value> = sessions(list)
        .iter()
        .filter(|s| s["current"] == json!(true))
        .collect();
    assert_eq!(mine.len(), 1, "one current session: {list}");
    mine[0].clone()
}

/// A sign-up records when its passkey was added and signed in; every later
/// sign-in moves the last sign-in, a refresh does not, and an added passkey is
/// added but not signed in until it is.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn sign_ins_move_a_passkeys_last_sign_in_and_refreshes_do_not() {
    let c = Cluster::start(3).await;
    let (laptop, mut pk) = signed_up(&c, 0, "alice").await;
    let me = laptop.me().await.unwrap();
    let cred = me["passkeyId"].as_str().unwrap().to_owned();
    let added = passkey(&me, &cred)["addedAt"].as_u64().expect("addedAt");
    assert_eq!(passkey(&me, &cred)["lastSignInAt"], json!(added), "{me}");

    // A second sign-in, through another node, a second later.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let phone = c.client(1);
    let _s = phone.sign_in(&mut pk).await.unwrap();
    let me = phone.me().await.unwrap();
    let first = passkey(&me, &cred)["lastSignInAt"].as_u64().unwrap();
    assert!(first > added, "{me}");
    assert_eq!(passkey(&me, &cred)["addedAt"], json!(added));

    // A third: the last one wins, on every node.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let tablet = c.client(2);
    let _s = tablet.sign_in(&mut pk).await.unwrap();
    let second = passkey(&tablet.me().await.unwrap(), &cred)["lastSignInAt"]
        .as_u64()
        .unwrap();
    assert!(second > first);

    // A refresh (the harness exchanges every one in the log) is no sign-in,
    // and keeps when its session opened.
    let opened = current(&laptop.sessions().await.unwrap())["openedAt"].clone();
    assert_eq!(opened, json!(added), "a sign-up opens its session");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let _s = laptop.refresh().await.unwrap();
    for i in 0..3 {
        let at = c.client(i);
        at.use_session(laptop.session().await.unwrap()).await;
        let me = at.me().await.unwrap();
        assert_eq!(
            passkey(&me, &cred)["lastSignInAt"],
            json!(second),
            "node {i}"
        );
    }
    let after = current(&laptop.sessions().await.unwrap());
    assert_eq!(after["openedAt"], opened, "{after}");
    assert!(after["lastActiveAt"].as_u64().unwrap() > added, "{after}");

    // An added passkey: added now, never signed in, until it signs in.
    let mut key = SoftAuthenticator::new("localhost", ORIGIN);
    let new = laptop.add_passkey(&mut key, &mut pk).await.unwrap();
    let me = laptop.me().await.unwrap();
    assert!(passkey(&me, &new)["addedAt"].as_u64().unwrap() >= second);
    assert_eq!(passkey(&me, &new)["lastSignInAt"], Value::Null, "{me}");
    let other = c.client(1);
    let _s = other.sign_in(&mut key).await.unwrap();
    let me = other.me().await.unwrap();
    assert!(
        passkey(&me, &new)["lastSignInAt"].as_u64().is_some(),
        "{me}"
    );
    assert_eq!(passkey(&me, &cred)["lastSignInAt"], json!(second));

    // A removed passkey takes its times with it.
    let _ok = laptop.remove_passkey(&new, &mut pk).await.unwrap();
    let removed = c.nodes[0].as_ref().unwrap().node.sys(|s| {
        s.accounts
            .values()
            .any(|a| a.passkey_times.contains_key(&new))
    });
    assert!(!removed, "the removed passkey's times are gone");
}

/// The account's sessions are listed with the passkey each was opened with,
/// when it opened and was last active, and which one asks; one ended through
/// the list stops refreshing and is no longer listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_second_session_is_listed_and_ended_on_its_own() {
    let c = Cluster::start(3).await;
    let (laptop, mut pk) = signed_up(&c, 0, "alice").await;
    let cred = laptop.me().await.unwrap()["passkeyId"].clone();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut pk).await.unwrap();

    let list = laptop.sessions().await.unwrap();
    assert_eq!(sessions(&list).len(), 2, "{list}");
    let mine = current(&list);
    for s in sessions(&list) {
        assert_eq!(s["passkey"], cred, "{s}");
        assert_eq!(s["app"], Value::Null);
        assert_eq!(s["origin"], Value::Null);
        assert!(s["openedAt"].as_u64().is_some(), "{s}");
        assert!(s["lastActiveAt"].as_u64().is_some(), "{s}");
    }
    let phones = current(&phone.sessions().await.unwrap());
    assert_ne!(phones["session"], mine["session"]);

    // The laptop ends the phone's session, through another node.
    let elsewhere = c.client(2);
    elsewhere.use_session(laptop.session().await.unwrap()).await;
    let _ok = elsewhere
        .revoke_session(phones["session"].as_str().unwrap())
        .await
        .unwrap();
    let e = phone.refresh().await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
    let e = phone.me().await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
    let list = laptop.sessions().await.unwrap();
    assert_eq!(sessions(&list).len(), 1, "{list}");
    assert_eq!(current(&list)["session"], mine["session"]);
    assert!(laptop.me().await.is_ok(), "the laptop's own session stays");

    // Ended already: there is nothing to end.
    let e = laptop
        .revoke_session(phones["session"].as_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("404"), "{e}");

    // Its own session too, as a sign-out.
    let _ok = laptop
        .revoke_session(mine["session"].as_str().unwrap())
        .await
        .unwrap();
    let e = laptop.refresh().await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
}

/// Sessions are the account holder's alone: another account's session is
/// neither listed nor ended (`404`, as one that does not exist), and an API
/// key of the account is refused the list and the end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn another_accounts_session_is_neither_listed_nor_ended() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;
    let bobs = current(&bob.sessions().await.unwrap());

    let list = alice.sessions().await.unwrap();
    assert_eq!(sessions(&list).len(), 1, "{list}");
    assert_ne!(current(&list)["session"], bobs["session"]);
    let e = alice
        .revoke_session(bobs["session"].as_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("404"), "{e}");
    let _s = bob.refresh().await.unwrap();
    assert!(bob.me().await.is_ok(), "bob's session stays");

    // An API key is the account's, but not its holder's session.
    let (_id, key) = alice
        .create_api_key(json!({ "label": "ci" }), &mut alice_pk)
        .await
        .unwrap();
    let machine = c.client(2);
    machine.use_api_key(&key).await;
    let e = machine.sessions().await.unwrap_err().to_string();
    assert!(e.starts_with("403"), "{e}");
    let e = machine
        .revoke_session(current(&list)["session"].as_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("403"), "{e}");
    assert!(alice.me().await.is_ok());
}
