//! An instance's registered origins: whoever manages an instance (its owner,
//! an admin of its organization) registers a site of their own for it, though
//! the package is someone else's (here the operator's catalog). A passkey
//! ceremony there opens a session bound to the instance's package that reaches
//! that instance alone; what the account may do in it is the instance's
//! grants'. Removing the origin, or deleting the instance, ends it.
//!
//! Builds the component kv fixture `apps/fixture-kv` from this checkout
//! (see `harness/cluster.rs`).

use calimero_client::types::TeamJoin;
use calimero_client::{Client, Request};
use calimero_identity::testing::SoftAuthenticator;
use reqwest::Method;
use serde_json::json;

use crate::cluster::publisher_binding;
use crate::harness;
use crate::package_origins::{assert_cors, into_catalog, release, status};

use harness::{signed_up, typed, wire, Cluster};

const KV: &str = "com.test.kv";
/// A customer's own site for one store.
const SITE: &str = "https://flags.acme.example";
const SITE_RP: &str = "flags.acme.example";

/// The flow a store's owner runs for a site of theirs: register it, let a
/// person who signed up there in, and that person's session reaches this store
/// and no other, though their account is granted on both.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_instances_origin_signs_people_in_to_that_instance_only() {
    let c = Cluster::start(3).await;
    into_catalog(&c, release(KV, "1.0.0")).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let org = wire(alice.orgs().create("acme", None, None).await.unwrap())["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let new_store = |name: &str| typed(json!({ "name": name, "package": KV, "org": org }));
    let store = alice
        .apps()
        .create(new_store("flags"))
        .await
        .unwrap()
        .app_id;
    let other = alice
        .apps()
        .create(new_store("other"))
        .await
        .unwrap()
        .app_id;
    let _ok = alice
        .apps()
        .call(
            &store,
            "set",
            Some(json!({ "key": "dark", "value": "on" })),
            None,
            None,
        )
        .await
        .unwrap();
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;

    // Nobody manages the operator's package, so its own origins are no one's to register.
    let e = alice
        .packages()
        .origins_add(KV, SITE, None, None, Some(&mut alice_pk))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    // Bob does not reach the store: as if it were not there.
    let e = bob
        .apps()
        .origin_add(&store, SITE, None, Some(&mut bob_pk))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    let e = alice
        .apps()
        .origin_add(&store, "https://flags.acme.co.uk", Some("co.uk"), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_RP_ID"), "a public suffix: {e}");
    let e = alice
        .transport()
        .json(
            Request::new(Method::POST, format!("apps/{store}/origins"))
                .json(json!({ "origin": SITE, "served": true })),
        )
        .await
        .unwrap_err();
    assert!(matches!(status(&e), 400 | 422), "never served: {e}");

    // The store's owner registers the site with her passkey (412 first).
    let e = alice
        .apps()
        .origin_add(&store, SITE, None, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let added = wire(
        alice
            .apps()
            .origin_add(&store, SITE, None, Some(&mut alice_pk))
            .await
            .unwrap(),
    );
    assert_eq!(
        (
            &added["origin"],
            &added["rpId"],
            &added["app"],
            &added["package"]
        ),
        (&json!(SITE), &json!(SITE_RP), &json!(store), &json!(KV)),
        "{added}"
    );
    let listed = wire(alice.apps().origins(&store).await.unwrap());
    assert_eq!(listed["origins"][0]["origin"], SITE, "{listed}");
    assert!(wire(alice.apps().origins(&other).await.unwrap())["origins"]
        .as_array()
        .unwrap()
        .is_empty());
    let e = alice
        .apps()
        .origin_add(&other, SITE, None, Some(&mut alice_pk))
        .await
        .unwrap_err();
    assert_eq!(
        e.code(),
        Some("ORIGIN_TAKEN"),
        "one instance holds a site: {e}"
    );
    assert_cors(&c.urls[2], SITE, true).await;

    // Carol signs up on the site: a passkey for its rp-id, a new account, a
    // session bound to the store's package and naming the site.
    let carol = c.client(2);
    let mut carol_pk = SoftAuthenticator::new(SITE_RP, SITE);
    let s = carol
        .sign_up_at(&mut carol_pk, "carol", Some(SITE))
        .await
        .unwrap();
    let claims = calimero_identity::peek_session(&s.session_token).unwrap();
    assert_eq!(
        (claims.app.as_deref(), claims.origin.as_deref()),
        (Some(KV), Some(SITE))
    );
    let carol_id = claims.account;
    let get = || json!({ "key": "dark" });
    let e = carol
        .apps()
        .view(&store, "get", Some(get()), None, None)
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "not let in yet: {e}");

    // Alice lets her in to both stores, as a guest of the organization.
    let _ok = alice
        .orgs()
        .set_member(&org, &carol_id, typed(json!("guest")), Some(&mut alice_pk))
        .await
        .unwrap();
    for app in [&store, &other] {
        let grants = typed(json!([{ "kind": "org" }, { "kind": "account", "account": carol_id }]));
        let _ok = alice.apps().access(app, grants).await.unwrap();
    }
    let read = wire(
        carol
            .apps()
            .view(&store, "get", Some(get()), None, None)
            .await
            .unwrap(),
    );
    assert_eq!(read["result"], "on", "{read}");
    // Her site session reaches that store alone: not the other, not a new
    // instance, not the account's credentials.
    let e = carol
        .apps()
        .view(&other, "get", Some(get()), None, None)
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    let listed = wire(carol.apps().list(None, None).await.unwrap());
    let ids: Vec<&str> = listed["apps"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["appId"].as_str())
        .collect();
    assert_eq!(ids, [store.as_str()], "{listed}");
    let e = carol
        .apps()
        .create(typed(json!({ "name": "mine", "package": KV, "org": org })))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let e = carol
        .auth()
        .apikey_create(typed(json!({ "label": "k" })), None)
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    // Made an admin of the organization, she still sees nothing of the other
    // store's from the site.
    let _ok = alice
        .orgs()
        .set_member(&org, &carol_id, typed(json!("admin")), Some(&mut alice_pk))
        .await
        .unwrap();
    let e = carol.apps().invite_list(&other).await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    let e = carol.apps().public_link_list(&other).await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    let _ok = carol.apps().invite_list(&store).await.unwrap();
    // Nor anything of the organization's, or of her account's past the store.
    let team = wire(
        alice
            .orgs()
            .team_create(&org, "eng", TeamJoin::Invite, None)
            .await
            .unwrap(),
    )["teamId"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = [
        carol.orgs().get(&org, None, None, None).await.map(drop),
        carol.orgs().invite_list(&org).await.map(drop),
        carol.orgs().team_get(&org, &team).await.map(drop),
        carol.orgs().app_team_list(&org).await.map(drop),
        carol.orgs().usage(&org).await.map(drop),
        carol.orgs().policy(&org).await.map(drop),
        carol.orgs().upgrade_status(&org, "run").await.map(drop),
        carol.auth().usage().await.map(drop),
        carol.auth().alias_list().await.map(drop),
        carol.apps().undelegate(&store, &other).await.map(drop),
    ];
    for (i, r) in refused.into_iter().enumerate() {
        let e = r.unwrap_err();
        assert_eq!(status(&e), 403, "route {i}: {e}");
    }
    // Her account as the site reads it: the store's organization alone, none
    // of her passkeys and no recovery state.
    let beta = wire(alice.orgs().create("beta", None, None).await.unwrap())["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = alice
        .orgs()
        .set_member(
            &beta,
            &carol_id,
            typed(json!("member")),
            Some(&mut alice_pk),
        )
        .await
        .unwrap();
    let me = wire(carol.auth().me().await.unwrap());
    let orgs: Vec<&str> = me["orgs"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|o| o["orgId"].as_str())
        .collect();
    assert_eq!(orgs, [org.as_str()], "{me}");
    assert_eq!(
        (&me["passkeys"], &me["recoveryCode"], &me["recoveryPending"]),
        (&json!([]), &json!(false), &json!(null)),
        "{me}"
    );
    // Nor does the site ask her for anything past the store: an organization of hers.
    let e = carol
        .transport()
        .json(Request::new(Method::POST, "consent/requests").json(json!({
            "kind": "createOrg",
            "subject": { "name": "mine" },
            "returnTo": format!("{SITE}/"),
        })))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    // An invite it may ask her to accept is one to the store alone.
    let accept = |token: String| {
        Request::new(Method::POST, "consent/requests").json(json!({
            "kind": "acceptInvite",
            "subject": { "token": token },
            "returnTo": format!("{SITE}/"),
        }))
    };
    let to_other = alice
        .apps()
        .invite_create(&other, None, None, None, Some(&mut alice_pk))
        .await
        .unwrap()
        .token;
    let to_org = alice
        .orgs()
        .invite_create(
            &org,
            typed(json!({ "role": "member" })),
            Some(&mut alice_pk),
        )
        .await
        .unwrap()
        .token;
    for token in [to_other, to_org] {
        let e = carol.transport().json(accept(token)).await.unwrap_err();
        assert_eq!(status(&e), 403, "{e}");
    }
    let to_store = alice
        .apps()
        .invite_create(&store, None, None, None, Some(&mut alice_pk))
        .await
        .unwrap()
        .token;
    let _made = carol.transport().json(accept(to_store)).await.unwrap();
    // Bob's own valid consent over the change, logged without the API, is refused at apply.
    let statement = calimero_node::proof::Statement::new(
        &store,
        "__add_origin",
        &json!({ "origin": "https://evil.example", "rpId": "evil.example" }),
    );
    let forged = json!({
        "op": "addAppOrigin",
        "app_id": store,
        "origin": "https://evil.example",
        "rp_id": "evil.example",
        "actor": wire(bob.auth().me().await.unwrap())["accountId"],
        "at": 1,
        "consent": calimero_node::proof::Consent {
            assertion: bob_pk.assert(&statement.challenge().unwrap()),
            statement,
            at: 0,
        },
    });
    let refused = crate::harness::append_routed(&c, &serde_json::to_value(&forged).unwrap())
        .await
        .unwrap_err();
    assert!(refused.contains("manages"), "{refused}");

    // Removal ends her site session; the site may not call the node any more.
    let removed = wire(
        alice
            .apps()
            .origin_remove(&store, SITE, Some(&mut alice_pk))
            .await
            .unwrap(),
    );
    assert_eq!(removed["removed"], true);
    let e = carol
        .apps()
        .view(&store, "get", Some(get()), None, None)
        .await
        .unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
    assert_cors(&c.urls[0], SITE, false).await;

    // Registered again, the site does not bring her session back: it was
    // opened under the registration that was removed.
    let _ok = alice
        .apps()
        .origin_add(&store, SITE, None, Some(&mut alice_pk))
        .await
        .unwrap();
    let e = carol
        .apps()
        .view(&store, "get", Some(get()), None, None)
        .await
        .unwrap_err();
    assert_eq!(status(&e), 401, "{e}");

    // An instance deleted takes its origin with it: the site is free again,
    // and registered for another instance it reaches nothing of hers.
    let _gone = alice.apps().delete(&store).await.unwrap();
    let _ok = alice
        .apps()
        .origin_add(&other, SITE, None, Some(&mut alice_pk))
        .await
        .unwrap();
    let e = carol
        .apps()
        .view(&other, "get", Some(get()), None, None)
        .await
        .unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
    let e = carol.refresh().await.unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
}

/// An instance's origin removed and registered again as its package's own
/// does not hand the sessions opened under the first registration the package.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_sessions_registration_ends_with_it_though_the_origin_returns() {
    const OWN: &str = "com.test.own";
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let org = crate::common::home_org(&alice).await;
    let _ok = alice
        .packages()
        .upload(release(OWN, "1.0.0"), Some(&org))
        .await
        .unwrap();
    into_catalog(&c, release(OWN, "1.0.0")).await;
    let store = alice
        .apps()
        .create(typed(
            json!({ "name": "flags", "package": OWN, "org": org }),
        ))
        .await
        .unwrap()
        .app_id;
    let _ok = alice
        .apps()
        .origin_add(&store, SITE, None, Some(&mut alice_pk))
        .await
        .unwrap();
    let carol = c.client(2);
    let mut carol_pk = SoftAuthenticator::new(SITE_RP, SITE);
    let _s = carol
        .sign_up_at(&mut carol_pk, "carol", Some(SITE))
        .await
        .unwrap();
    let _me = carol.auth().me().await.unwrap();

    let _ok = alice
        .apps()
        .origin_remove(&store, SITE, Some(&mut alice_pk))
        .await
        .unwrap();
    let _ok = alice
        .packages()
        .origins_add(OWN, SITE, None, None, Some(&mut alice_pk))
        .await
        .unwrap();
    let e = carol.auth().me().await.unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
    let e = carol.refresh().await.unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
}

/// A package's bound publisher takes back its site from someone else's instance
/// of the package: the instance loses it, and the sessions opened there end.
/// Its first installer, before a publisher is bound, does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_packages_manager_takes_back_an_origin_an_instance_holds() {
    const OWN: &str = "com.test.own";
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let org = crate::common::home_org(&alice).await;
    let _ok = alice
        .packages()
        .upload(release(OWN, "1.0.0"), Some(&org))
        .await
        .unwrap();
    into_catalog(&c, release(OWN, "1.0.0")).await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let bobs = crate::common::home_org(&bob).await;
    let store = bob
        .apps()
        .create(typed(
            json!({ "name": "squat", "package": OWN, "org": bobs }),
        ))
        .await
        .unwrap()
        .app_id;
    let _ok = bob
        .apps()
        .origin_add(&store, SITE, None, Some(&mut bob_pk))
        .await
        .unwrap();
    let carol = c.client(2);
    let mut carol_pk = SoftAuthenticator::new(SITE_RP, SITE);
    let _s = carol
        .sign_up_at(&mut carol_pk, "carol", Some(SITE))
        .await
        .unwrap();
    let _me = carol.auth().me().await.unwrap();

    let e = alice
        .packages()
        .origins_add(OWN, SITE, None, None, Some(&mut alice_pk))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 409, "{e}");
    let _me = carol.auth().me().await.unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let _ok = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/publishers"))
                .json(publisher_binding(&alice, &key, &org).await),
            &mut alice_pk,
        )
        .await
        .unwrap();

    let added = wire(
        alice
            .packages()
            .origins_add(OWN, SITE, None, None, Some(&mut alice_pk))
            .await
            .unwrap(),
    );
    assert_eq!(
        (&added["package"], &added["app"]),
        (&json!(OWN), &json!(null)),
        "{added}"
    );
    assert!(wire(bob.apps().origins(&store).await.unwrap())["origins"]
        .as_array()
        .unwrap()
        .is_empty());
    let e = carol.auth().me().await.unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
}

/// An instance moved to another organization, or handed to a new owner,
/// leaves its origins behind: the former manager's site signs no one in to it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_instance_moved_or_transferred_loses_its_origins() {
    let c = Cluster::start(3).await;
    into_catalog(&c, release(KV, "1.0.0")).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;
    let org_of = |v: serde_json::Value| v["orgId"].as_str().unwrap().to_owned();
    let acme = org_of(wire(alice.orgs().create("acme", None, None).await.unwrap()));
    let beta = org_of(wire(alice.orgs().create("beta", None, None).await.unwrap()));
    let store = alice
        .apps()
        .create(typed(
            json!({ "name": "flags", "package": KV, "org": acme }),
        ))
        .await
        .unwrap()
        .app_id;

    let carol = signed_in_at_site(&c, &alice, &mut alice_pk, &store, "carol").await;
    let _moved = alice
        .apps()
        .app_move(&store, &beta, Some(false), Some(&mut alice_pk))
        .await
        .unwrap();
    assert_site_gone(&alice, &store, &carol).await;

    let dave = signed_in_at_site(&c, &alice, &mut alice_pk, &store, "dave").await;
    let token = alice
        .orgs()
        .invite_create(
            &beta,
            typed(json!({ "role": "member" })),
            Some(&mut alice_pk),
        )
        .await
        .unwrap()
        .token;
    let _joined = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = wire(bob.auth().me().await.unwrap())["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    alice
        .apps()
        .transfer(&store, &bob_id, Some(&mut alice_pk))
        .await
        .unwrap();
    assert_site_gone(&alice, &store, &dave).await;
}

/// `store`'s manager registers [`SITE`] for it, and `name` signs up there.
async fn signed_in_at_site(
    c: &Cluster,
    manager: &Client,
    pk: &mut SoftAuthenticator,
    store: &str,
    name: &str,
) -> Client {
    let _ok = manager
        .apps()
        .origin_add(store, SITE, None, Some(pk))
        .await
        .unwrap();
    let person = c.client(2);
    let mut site_pk = SoftAuthenticator::new(SITE_RP, SITE);
    let _s = person
        .sign_up_at(&mut site_pk, name, Some(SITE))
        .await
        .unwrap();
    let _me = person.auth().me().await.unwrap();
    person
}

/// `store` holds no origin, and `person`'s session from the site is over.
async fn assert_site_gone(manager: &Client, store: &str, person: &Client) {
    let listed = wire(manager.apps().origins(store).await.unwrap());
    assert!(listed["origins"].as_array().unwrap().is_empty(), "{listed}");
    let e = person.auth().me().await.unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
}
