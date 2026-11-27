//! What the account holder sees of their own passkeys and sessions: when each
//! passkey was added and last signed in (`/auth/me`), and the account's live
//! sessions, listed and ended one at a time (`/auth/sessions`). Nobody else
//! sees or ends them: another account's session is `404`, and an API key is
//! refused the list.

use std::time::Duration;

use calimero_identity::testing::SoftAuthenticator;
use serde_json::{json, Value};

use crate::harness;

use harness::{signed_up, typed, wire, Cluster, ORIGIN};

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
    let me = laptop.auth().me().await.map(wire).unwrap();
    let cred = me["passkeyId"].as_str().unwrap().to_owned();
    let added = passkey(&me, &cred)["addedAt"].as_u64().expect("addedAt");
    assert_eq!(passkey(&me, &cred)["lastSignInAt"], json!(added), "{me}");

    // A second sign-in, through another node, a second later.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let phone = c.client(1);
    let _s = phone.sign_in(&mut pk).await.unwrap();
    let me = phone.auth().me().await.map(wire).unwrap();
    let first = passkey(&me, &cred)["lastSignInAt"].as_u64().unwrap();
    assert!(first > added, "{me}");
    assert_eq!(passkey(&me, &cred)["addedAt"], json!(added));

    // A third: the last one wins, on every node.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let tablet = c.client(2);
    let _s = tablet.sign_in(&mut pk).await.unwrap();
    let second = passkey(&tablet.auth().me().await.map(wire).unwrap(), &cred)["lastSignInAt"]
        .as_u64()
        .unwrap();
    assert!(second > first);

    // A refresh (the harness exchanges every one in the log) is no sign-in,
    // and keeps when its session opened.
    let opened =
        current(&laptop.auth().sessions_list().await.map(wire).unwrap())["openedAt"].clone();
    assert_eq!(opened, json!(added), "a sign-up opens its session");
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let _s = laptop.refresh().await.unwrap();
    for i in 0..3 {
        let at = c.client(i);
        at.use_session(laptop.session().await.unwrap()).await;
        let me = at.auth().me().await.map(wire).unwrap();
        assert_eq!(
            passkey(&me, &cred)["lastSignInAt"],
            json!(second),
            "node {i}"
        );
    }
    let after = current(&laptop.auth().sessions_list().await.map(wire).unwrap());
    assert_eq!(after["openedAt"], opened, "{after}");
    assert!(after["lastActiveAt"].as_u64().unwrap() > added, "{after}");

    // An added passkey: added now, never signed in, until it signs in.
    let mut key = SoftAuthenticator::new("localhost", ORIGIN);
    let new = laptop.add_passkey(&mut key, &mut pk).await.unwrap();
    let me = laptop.auth().me().await.map(wire).unwrap();
    assert!(passkey(&me, &new)["addedAt"].as_u64().unwrap() >= second);
    assert_eq!(passkey(&me, &new)["lastSignInAt"], Value::Null, "{me}");
    let other = c.client(1);
    let _s = other.sign_in(&mut key).await.unwrap();
    let me = other.auth().me().await.map(wire).unwrap();
    assert!(
        passkey(&me, &new)["lastSignInAt"].as_u64().is_some(),
        "{me}"
    );
    assert_eq!(passkey(&me, &cred)["lastSignInAt"], json!(second));

    // A removed passkey takes its times with it.
    let _ok = laptop
        .auth()
        .passkey_remove(&new, Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
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
    let cred = laptop.auth().me().await.map(wire).unwrap()["passkeyId"].clone();
    let phone = c.client(1);
    let _s = phone.sign_in(&mut pk).await.unwrap();

    let list = laptop.auth().sessions_list().await.map(wire).unwrap();
    assert_eq!(sessions(&list).len(), 2, "{list}");
    let mine = current(&list);
    for s in sessions(&list) {
        assert_eq!(s["passkey"], cred, "{s}");
        assert_eq!(s["app"], Value::Null);
        assert_eq!(s["origin"], Value::Null);
        assert!(s["openedAt"].as_u64().is_some(), "{s}");
        assert!(s["lastActiveAt"].as_u64().is_some(), "{s}");
    }
    let phones = current(&phone.auth().sessions_list().await.map(wire).unwrap());
    assert_ne!(phones["session"], mine["session"]);

    // The laptop ends the phone's session, through another node.
    let elsewhere = c.client(2);
    elsewhere.use_session(laptop.session().await.unwrap()).await;
    let _ok = elsewhere
        .auth()
        .session_revoke(phones["session"].as_str().unwrap())
        .await
        .unwrap();
    let e = phone.refresh().await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
    let e = phone.auth().me().await.unwrap_err().to_string();
    assert!(e.starts_with("401"), "{e}");
    let list = laptop.auth().sessions_list().await.map(wire).unwrap();
    assert_eq!(sessions(&list).len(), 1, "{list}");
    assert_eq!(current(&list)["session"], mine["session"]);
    assert!(
        laptop.auth().me().await.is_ok(),
        "the laptop's own session stays"
    );

    // Ended already: there is nothing to end.
    let e = laptop
        .auth()
        .session_revoke(phones["session"].as_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("404"), "{e}");

    // Its own session too, as a sign-out.
    let _ok = laptop
        .auth()
        .session_revoke(mine["session"].as_str().unwrap())
        .await
        .map(wire)
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
    let bobs = current(&bob.auth().sessions_list().await.map(wire).unwrap());

    let list = alice.auth().sessions_list().await.map(wire).unwrap();
    assert_eq!(sessions(&list).len(), 1, "{list}");
    assert_ne!(current(&list)["session"], bobs["session"]);
    let e = alice
        .auth()
        .session_revoke(bobs["session"].as_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("404"), "{e}");
    let _s = bob.refresh().await.unwrap();
    assert!(bob.auth().me().await.is_ok(), "bob's session stays");

    // An API key is the account's, but not its holder's session.
    let (_id, key) = alice
        .auth()
        .apikey_create(typed(json!({ "label": "ci" })), Some(&mut alice_pk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let machine = c.client(2);
    machine.use_api_key(&key).await;
    let e = machine
        .auth()
        .sessions_list()
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("403"), "{e}");
    let e = machine
        .auth()
        .session_revoke(current(&list)["session"].as_str().unwrap())
        .await
        .unwrap_err()
        .to_string();
    assert!(e.starts_with("403"), "{e}");
    assert!(alice.auth().me().await.is_ok());
}

/// A flood of sign-in begins from one sealed session never stops another
/// client's sign-in on the same node: the flooding session holds at most
/// `MAX_CEREMONIES_PER_SESSION` open ceremonies, its next begin displacing
/// its own oldest, so the pool other callers share stays open.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_flood_of_sign_in_begins_does_not_stop_another_clients_sign_in() {
    use calimero_client::Passkey;
    use futures_util::StreamExt;

    let c = Cluster::start(1).await;
    let (_alice, mut pk) = signed_up(&c, 0, "alice").await;

    // One sealed session begins as many sign-ins as the node's whole pool holds.
    let flood = c.client(0);
    let flooding = &flood;
    let begun: Vec<Value> =
        futures_util::stream::iter(0..calimero_node::api::MAX_PENDING_CHALLENGES)
            .map(|_| async move { flooding.auth().login_begin(None, None).await.map(wire) })
            .buffer_unordered(16)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<_, _>>()
            .expect("a session's begins are never refused: each displaces its own oldest");

    // Another client still signs in.
    let other = c.client(0);
    let _s = other
        .sign_in(&mut pk)
        .await
        .expect("a flood from one session leaves the others room");

    // The flood's earliest ceremonies were displaced by its later ones.
    let first = &begun[0];
    let challenge =
        calimero_identity::from_b64url(first["options"]["challenge"].as_str().unwrap()).unwrap();
    let e = flood
        .transport()
        .json(
            calimero_client::Request::new(reqwest::Method::POST, "auth/passkey/login/finish").json(
                json!({
                    "challengeId": first["challengeId"],
                    "assertion": pk.get(&challenge),
                }),
            ),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UNKNOWN_CHALLENGE"), "{e}");
}

/// A step-up signed with a stale copy of the passkey, whose counter is behind
/// the node's (two processes sharing one passkey file sign with the same
/// count), is refused with what went wrong, `403 SIGN_COUNT_REPLAYED`, and not
/// a `401`: the client would take that for an expired session, spend a
/// refresh, and send the spent challenge again (`400 UNKNOWN_CHALLENGE`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_passkey_counter_is_named_and_spends_no_refresh() {
    let c = Cluster::start(1).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let mut stale = SoftAuthenticator::from_json(&pk.to_json()).unwrap();
    let key = |label: &str| {
        calimero_client::Request::new(reqwest::Method::POST, "auth/apikeys")
            .json(json!({ "label": label }))
    };
    let _made = alice
        .transport()
        .confirmed(key("first"), &mut pk)
        .await
        .unwrap();
    let before = alice.transport().session().await.unwrap();

    let e = alice
        .transport()
        .confirmed(key("second"), &mut stale)
        .await
        .unwrap_err();
    let calimero_client::Error::Api { status, code, .. } = &e else {
        panic!("not the node's answer: {e}");
    };
    assert_eq!(
        (*status, code.as_str()),
        (403, "SIGN_COUNT_REPLAYED"),
        "{e}"
    );
    let after = alice.transport().session().await.unwrap();
    assert_eq!(
        after.session_token, before.session_token,
        "no refresh spent"
    );

    // The current passkey still approves.
    let _made = alice
        .transport()
        .confirmed(key("third"), &mut pk)
        .await
        .unwrap();
}

/// A sign-up's name is held with its ceremony until the challenge expires, so
/// it is bounded: one past `MAX_ACCOUNT_NAME_CHARS` is refused `400` before
/// anything is held, a body past `MAX_PASSKEY_CEREMONY_BYTES` is refused
/// `413`, and a name at the limit still begins.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sign_up_name_past_the_limit_is_refused_and_nothing_is_held() {
    use calimero_node::api::{MAX_ACCOUNT_NAME_CHARS, MAX_PASSKEY_CEREMONY_BYTES};

    let c = Cluster::start(1).await;
    let held = || {
        c.nodes[0]
            .as_ref()
            .unwrap()
            .node
            .challenges
            .lock()
            .unwrap()
            .len()
    };
    let client = c.client(0);

    let long = "n".repeat(MAX_ACCOUNT_NAME_CHARS + 1);
    let e = client
        .auth()
        .register_begin(Some(&long), None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NAME_TOO_LONG"), "{e}");
    assert!(e.to_string().starts_with("400"), "{e}");
    assert_eq!(held(), 0, "a refused begin holds nothing");

    let huge = "n".repeat(MAX_PASSKEY_CEREMONY_BYTES);
    let e = client
        .auth()
        .register_begin(Some(&huge), None, None)
        .await
        .unwrap_err();
    assert!(e.to_string().starts_with("413"), "{e}");
    assert_eq!(held(), 0, "a refused begin holds nothing");

    let fits = "é".repeat(MAX_ACCOUNT_NAME_CHARS);
    let _begun = client
        .auth()
        .register_begin(Some(&fits), None, None)
        .await
        .expect("a name at the limit begins");
    assert_eq!(held(), 1);
}
