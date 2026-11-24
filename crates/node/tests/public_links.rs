//! Public links (`Feature::PublicLinks`, 1.0.0-rc.33): an instance's managers make a link
//! whose token lets anonymous visitors, each with its own Ed25519 key, call the instance's
//! `#[app::public]` methods as `Caller::Visitor { link, key }`, whatever the instance's access,
//! and nothing else; the organization's `allowPublicLinks`, revocation, expiry and a per-link
//! rate hold at the visitor's next call.
//!
//! Builds the component kv fixture `apps/fixture-kv` from this checkout
//! (see `harness/cluster.rs`).

#![allow(
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]
#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use calimero_client::types::ViewsRequestConsistency;
use calimero_client::visitor::Visitor;
use calimero_client::{Client, Error};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, typed, wire, Cluster};

fn s(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not a string: {v}"))
        .to_owned()
}

/// A client of node `i` calling as `visitor`.
async fn as_visitor(c: &Cluster, i: usize, visitor: &Visitor) -> Client {
    let client = c.client(i);
    client.use_visitor(visitor.clone()).await;
    client
}

async fn sign(c: &Client, app: &str, note: &str) -> Result<Value, Error> {
    c.apps()
        .call(
            app,
            "signGuestbook",
            Some(json!({ "note": note })),
            None,
            None,
        )
        .await
        .map(|r| r.result)
}

fn code(e: &Error) -> String {
    e.to_string()
}

/// A visitor through a link calls the public method of a private (organization-access)
/// instance and the app names it by link and key; two visitors have two keys; it is refused
/// every other method and route; the token never appears again after it is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_visitor_calls_only_the_public_methods_of_its_links_instance() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "forms", "bundle": &bundle })))
        .await
        .unwrap()
        .app_id;
    let other = alice
        .apps()
        .create(typed(json!({ "name": "other", "bundle": &bundle })))
        .await
        .unwrap()
        .app_id;
    let record = alice.apps().get(&app).await.map(wire).unwrap();
    assert_eq!(
        record["grants"],
        json!([{ "kind": "org" }]),
        "a private instance: {record}"
    );

    // No passkey, no link.
    let e = alice
        .apps()
        .public_link_create(&app, Some("Survey"), None, Some(5), None)
        .await
        .unwrap_err();
    assert!(code(&e).contains("CONFIRM_REQUIRED"), "{e}");
    let made = alice
        .apps()
        .public_link_create(&app, Some("Survey"), None, Some(5), Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    let token = s(&made["token"]);
    let link = s(&made["link"]);
    assert_eq!(calimero_client::visitor::link_key(&token), link);
    let listed = alice.apps().public_link_list(&app).await.map(wire).unwrap();
    assert_eq!(listed["links"][0]["link"], json!(link), "{listed}");
    assert_eq!(listed["links"][0]["ratePerMinute"], 5, "{listed}");
    assert!(
        !listed.to_string().contains(&token),
        "no token after it is made"
    );

    let one = Visitor::new(&token, [1; 32]);
    let two = Visitor::new(&token, [2; 32]);
    let v1 = as_visitor(&c, 1, &one).await;
    let v2 = as_visitor(&c, 2, &two).await;
    assert_eq!(
        sign(&v1, &app, "hello").await.unwrap(),
        json!(format!("visitor:{link}:{}", one.public_key()))
    );
    assert_eq!(
        sign(&v2, &app, "hi").await.unwrap(),
        json!(format!("visitor:{link}:{}", two.public_key()))
    );
    assert_ne!(one.public_key(), two.public_key());
    let seen = v1
        .apps()
        .view(
            &app,
            "whoamiPublic",
            Some(json!({})),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result;
    assert_eq!(seen, json!(format!("visitor:{link}:{}", one.public_key())));
    // The owner reads what the visitor wrote, under its name.
    let wrote = alice
        .apps()
        .view(
            &app,
            "get",
            Some(json!({ "key": format!("sign:visitor:{link}:{}", one.public_key()) })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .unwrap()
        .result;
    assert_eq!(wrote, json!("hello"));

    // Nothing else: a method that is not public, a view that is not, another app, any
    // other route.
    let e = v1
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v" })),
            None,
            None,
        )
        .await
        .unwrap_err();
    assert!(code(&e).starts_with("403"), "{e}");
    let e = v1
        .apps()
        .view(&app, "get", Some(json!({ "key": "k" })), None, None)
        .await
        .unwrap_err();
    assert!(code(&e).starts_with("403"), "{e}");
    let e = sign(&v1, &other, "x").await.unwrap_err();
    assert!(code(&e).starts_with("404"), "{e}");
    for e in [
        v1.apps().get(&app).await.map(drop).unwrap_err(),
        v1.apps().list(None, None).await.map(drop).unwrap_err(),
        v1.apps().abi(&app).await.map(drop).unwrap_err(),
        v1.auth().me().await.map(drop).unwrap_err(),
        v1.apps()
            .public_link_list(&app)
            .await
            .map(drop)
            .unwrap_err(),
    ] {
        assert!(code(&e).starts_with("403"), "{e}");
    }
    // A forged key (a proof another key signed) is no visitor.
    let forged = c.client(0);
    let bearer = one.bearer(calimero_identity_now());
    let mut parts: Vec<&str> = bearer.split('.').collect();
    let two_key = two.public_key();
    parts[2] = &two_key;
    forged.use_api_key(&parts.join(".")).await;
    let e = sign(&forged, &app, "x").await.unwrap_err();
    assert!(code(&e).starts_with("401"), "{e}");
}

fn calimero_identity_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// A link stops admitting visitors at their next call once it is revoked, expires, or its
/// organization turns public links off (and no new link is made then); each visitor gets the
/// link's rate a minute on a node.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_link_is_revoked_expired_switched_off_and_rate_limited() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "forms", "bundle": &bundle })))
        .await
        .unwrap()
        .app_id;
    let org = s(&alice.apps().get(&app).await.map(wire).unwrap()["org"]);
    let token = s(&alice
        .apps()
        .public_link_create(&app, None, None, Some(3), Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    let visitor = Visitor::new(&token, [3; 32]);
    let v = as_visitor(&c, 0, &visitor).await;

    // The rate: three calls a minute through this link for this visitor on this node.
    for i in 0..3 {
        sign(&v, &app, &format!("n{i}")).await.unwrap();
    }
    let e = sign(&v, &app, "n4").await.unwrap_err();
    assert!(code(&e).contains("TOO_MANY_VISITS"), "{e}");
    // Another visitor has its own allowance.
    let other = as_visitor(&c, 0, &Visitor::new(&token, [4; 32])).await;
    sign(&other, &app, "o").await.unwrap();

    // The organization turns public links off: refused at once, and no new link; on again.
    let _off = alice
        .orgs()
        .settings(&org, None, Some(false), Some(&mut a))
        .await
        .unwrap();
    let e = sign(&other, &app, "off").await.unwrap_err();
    assert!(code(&e).starts_with("401"), "{e}");
    let e = alice
        .apps()
        .public_link_create(&app, None, None, None, None)
        .await
        .unwrap_err();
    assert!(code(&e).contains("PUBLIC_LINKS_OFF"), "{e}");
    let _on = alice
        .orgs()
        .settings(&org, None, Some(true), Some(&mut a))
        .await
        .unwrap();
    sign(&other, &app, "on").await.unwrap();

    // Revoked: refused at the next call.
    let link = visitor.link();
    let _ok = alice
        .apps()
        .public_link_revoke(&app, &link, Some(&mut a))
        .await
        .unwrap();
    let e = sign(&other, &app, "gone").await.unwrap_err();
    assert!(code(&e).starts_with("401"), "{e}");

    // Expired: a link that ends in a few seconds admits a visitor, then nobody.
    let ends = calimero_identity_now() + 6;
    let short = s(&alice
        .apps()
        .public_link_create(&app, None, Some(ends), None, Some(&mut a))
        .await
        .map(wire)
        .unwrap()["token"]);
    let late = as_visitor(&c, 1, &Visitor::new(&short, [5; 32])).await;
    sign(&late, &app, "in time").await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(7)).await;
    let e = sign(&late, &app, "late").await.unwrap_err();
    assert!(code(&e).starts_with("401"), "{e}");
}
