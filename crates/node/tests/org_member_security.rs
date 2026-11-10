//! What an organization's owner and admins see of their members' lockout
//! risk (`GET /orgs/{org}/members?detail=security`): each member's name,
//! role and teams, a passkey count capped at `"3+"`, whether a recovery code
//! is set, the UTC day they were last active and how many live sessions they
//! hold. Nothing the account holder alone sees (credential ids, passkey
//! labels and times, sessions, origins, activity) and nobody outside the
//! organization; a member below admin is refused, and a removed member is
//! gone at once. A deployment's managers see only counts of its customer
//! organizations' members (`tests/deployments.rs`).

use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster, ORIGIN};

/// The keys a member's row may carry, and no other.
const KEYS: [&str; 8] = [
    "account",
    "name",
    "role",
    "teams",
    "passkeys",
    "recoveryCode",
    "lastActiveDay",
    "sessions",
];

async fn id(c: &Client) -> String {
    c.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn new_org(owner: &Client, name: &str) -> String {
    owner.post("orgs", json!({ "name": name })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `who` joins `org` at `role`, invited by `admin`.
async fn join(admin: &Client, pk: &mut SoftAuthenticator, org: &str, role: &str, who: &Client) {
    let token = admin
        .post_confirmed(&format!("orgs/{org}/invites"), json!({ "role": role }), pk)
        .await
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = who
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
}

#[allow(
    clippy::result_large_err,
    reason = "the client's own error type, as its `get` returns it"
)]
async fn members(c: &Client, org: &str) -> Result<Value, calimero_client::Error> {
    c.get(&format!("orgs/{org}/members?detail=security")).await
}

/// The listed row of `account`, if any.
fn row(list: &Value, account: &str) -> Option<Value> {
    list["members"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["account"] == json!(account))
        .cloned()
}

fn status(e: &calimero_client::Error) -> String {
    e.to_string()
}

/// The owner and an admin see every member's lockout facts and nothing the
/// member alone sees; a member is refused; a removed member is gone; an API
/// key of the owner is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn admins_see_their_members_lockout_risk_and_nothing_more() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let (carol, _carol_pk) = signed_up(&c, 2, "carol").await;
    let (alice_id, bob_id, carol_id) = (id(&alice).await, id(&bob).await, id(&carol).await);
    let org = new_org(&alice, "Acme").await;
    join(&alice, &mut alice_pk, &org, "member", &bob).await;
    join(&alice, &mut alice_pk, &org, "admin", &carol).await;
    let _ok = bob
        .set_member_metadata(&org, &bob_id, json!({ "name": "Bob B." }))
        .await
        .unwrap();
    // Bob names his device: a label is his alone.
    let bob_cred = bob.me().await.unwrap()["passkeyId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = bob
        .label_passkey(&bob_cred, "Bob's work laptop")
        .await
        .unwrap();

    for admin in [&alice, &carol] {
        let list = members(admin, &org).await.unwrap();
        assert_eq!(list["members"].as_array().unwrap().len(), 3, "{list}");
        let b = row(&list, &bob_id).unwrap();
        assert_eq!(b["name"], "Bob B.", "{b}");
        assert_eq!(b["role"], "member");
        assert_eq!(b["passkeys"], 1);
        assert_eq!(b["recoveryCode"], false);
        assert_eq!(b["sessions"], 1);
        let day = b["lastActiveDay"].as_str().expect("lastActiveDay");
        assert_eq!(day.len(), 10, "a UTC day, not a time: {day}");
        for m in list["members"].as_array().unwrap() {
            for k in m.as_object().unwrap().keys() {
                assert!(KEYS.contains(&k.as_str()), "{k} in {m}");
            }
        }
        let text = list.to_string();
        assert!(!text.contains(&bob_cred), "no credential id: {text}");
        assert!(!text.contains("work laptop"), "no passkey label: {text}");
        assert_eq!(row(&list, &alice_id).unwrap()["role"], "owner");
        assert_eq!(row(&list, &carol_id).unwrap()["role"], "admin");
    }

    // More passkeys and a recovery code: counted to "3+", the code a yes.
    for _ in 0..2 {
        let mut key = SoftAuthenticator::new("localhost", ORIGIN);
        let _cred = bob.add_passkey(&mut key, &mut bob_pk).await.unwrap();
    }
    let _code = bob
        .post_confirmed("auth/recovery", json!({}), &mut bob_pk)
        .await
        .unwrap();
    let b = row(&members(&alice, &org).await.unwrap(), &bob_id).unwrap();
    assert_eq!(b["passkeys"], "3+", "{b}");
    assert_eq!(b["recoveryCode"], true, "{b}");

    // A member is refused; so is the owner's API key.
    let e = members(&bob, &org).await.unwrap_err();
    assert!(status(&e).starts_with("403"), "{e}");
    let (_key_id, key) = alice
        .create_api_key(json!({ "label": "ci" }), &mut alice_pk)
        .await
        .unwrap();
    let machine = c.client(1);
    machine.use_api_key(&key).await;
    let e = members(&machine, &org).await.unwrap_err();
    assert!(status(&e).starts_with("403"), "{e}");
    // Without `detail=security` there is no listing.
    let e = alice.get(&format!("orgs/{org}/members")).await.unwrap_err();
    assert!(status(&e).starts_with("400"), "{e}");

    // Removed: gone from the listing, and no longer a reader of it.
    let _ok = alice
        .delete_confirmed(&format!("orgs/{org}/members/{bob_id}"), &mut alice_pk)
        .await
        .unwrap();
    let list = members(&carol, &org).await.unwrap();
    assert!(row(&list, &bob_id).is_none(), "{list}");
    assert_eq!(list["members"].as_array().unwrap().len(), 2, "{list}");
    let e = members(&bob, &org).await.unwrap_err();
    assert!(status(&e).starts_with("404"), "{e}");
}

/// Another organization's members never show: each admin lists their own
/// organization only, and someone else's is answered as if it did not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn another_organizations_members_never_show() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;
    let (dave, mut dave_pk) = signed_up(&c, 2, "dave").await;
    let (erin, _erin_pk) = signed_up(&c, 0, "erin").await;
    let (bob_id, erin_id) = (id(&bob).await, id(&erin).await);
    let acme = new_org(&alice, "Acme").await;
    let globex = new_org(&dave, "Globex").await;
    join(&alice, &mut alice_pk, &acme, "member", &bob).await;
    join(&dave, &mut dave_pk, &globex, "member", &erin).await;

    let list = members(&alice, &acme).await.unwrap();
    assert!(row(&list, &bob_id).is_some(), "{list}");
    assert!(row(&list, &erin_id).is_none(), "{list}");
    let list = members(&dave, &globex).await.unwrap();
    assert!(row(&list, &erin_id).is_some(), "{list}");
    assert!(row(&list, &bob_id).is_none(), "{list}");

    for (who, org) in [(&alice, &globex), (&dave, &acme), (&erin, &acme)] {
        let e = members(who, org).await.unwrap_err();
        assert!(status(&e).starts_with("404"), "{e}");
    }
    let e = members(&alice, "no-such-org").await.unwrap_err();
    assert!(status(&e).starts_with("404"), "{e}");
}
