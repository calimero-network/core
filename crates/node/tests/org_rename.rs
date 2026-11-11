//! Renaming an organization (`POST /orgs/{org}/name { name }`,
//! `SystemCmd::RenameOrg`): its owner or an admin, with a passkey (`412`
//! first), to a name that fits as a new organization's must. The new name is
//! what every reader of it answers: `GET /orgs/{org}`, `/auth/me`'s `orgs`,
//! and an invite minted before the rename and redeemed after it. A member
//! below admin is refused, and someone outside it is answered as if it did
//! not exist.

use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster};

fn s(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not a string: {v}"))
        .to_owned()
}

/// An invite into `org` at `role`, made by `by` with `pk`: its token.
async fn invite(by: &Client, pk: &mut SoftAuthenticator, org: &str, role: &str) -> String {
    let v = by
        .post_confirmed(&format!("orgs/{org}/invites"), json!({ "role": role }), pk)
        .await
        .unwrap();
    s(&v["token"])
}

async fn redeem(c: &Client, token: &str) {
    let _r = c
        .post(&format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap();
}

/// The name `/auth/me` gives `org` for `c`.
async fn my_name_for(c: &Client, org: &str) -> Value {
    let me = c.me().await.unwrap();
    me["orgs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|o| o["orgId"] == org)
        .map(|o| o["name"].clone())
        .unwrap_or_else(|| panic!("{org} not in {me}"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_admin_renames_the_organization_with_a_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, mut b) = signed_up(&c, 1, "bob").await;
    let (carol, mut cpk) = signed_up(&c, 2, "carol").await;
    let (dave, _d) = signed_up(&c, 0, "dave").await;
    let (erin, mut e) = signed_up(&c, 1, "erin").await;
    let org = s(&alice.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]);
    redeem(&bob, &invite(&alice, &mut a, &org, "admin").await).await;
    redeem(&carol, &invite(&alice, &mut a, &org, "member").await).await;
    // Minted before the rename, redeemed after it.
    let for_dave = invite(&alice, &mut a, &org, "member").await;
    let path = format!("orgs/{org}/name");

    // A step-up first; without a passkey the request changes nothing.
    let err = bob
        .post(&path, json!({ "name": "Acme Labs" }))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("CONFIRM_REQUIRED"), "{err}");
    // A member below admin, and someone outside it.
    let err = carol
        .post_confirmed(&path, json!({ "name": "Carolco" }), &mut cpk)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("FORBIDDEN"), "{err}");
    let err = erin
        .post_confirmed(&path, json!({ "name": "Erinco" }), &mut e)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("NOT_FOUND"), "{err}");
    // Names that would not make an organization either.
    for bad in [
        String::new(),
        "   ".to_owned(),
        " Acme".to_owned(),
        "Ac\nme".to_owned(),
        "x".repeat(65),
    ] {
        let err = bob
            .post_confirmed(&path, json!({ "name": bad }), &mut b)
            .await
            .unwrap_err();
        assert_eq!(err.code(), Some("BAD_REQUEST"), "{bad:?}: {err}");
    }
    assert_eq!(
        alice.get(&format!("orgs/{org}")).await.unwrap()["name"],
        "Acme"
    );

    // An admin renames it; every reader answers the new name.
    let out = bob
        .post_confirmed(&path, json!({ "name": "Acme Labs" }), &mut b)
        .await
        .unwrap();
    assert_eq!(out["name"], "Acme Labs", "{out}");
    assert_eq!(out["orgId"], json!(org), "{out}");
    for p in [&alice, &bob, &carol] {
        assert_eq!(
            p.get(&format!("orgs/{org}")).await.unwrap()["name"],
            "Acme Labs"
        );
        assert_eq!(my_name_for(p, &org).await, "Acme Labs");
    }
    redeem(&dave, &for_dave).await;
    assert_eq!(my_name_for(&dave, &org).await, "Acme Labs");

    // The owner renames it too; 64 characters fit.
    let long = "y".repeat(64);
    let _ok = alice
        .post_confirmed(&path, json!({ "name": long }), &mut a)
        .await
        .unwrap();
    assert_eq!(my_name_for(&carol, &org).await, json!(long));
}
