//! Registered package origins: a package's manager registers a web origin
//! of their own (a developer's site, not one the node serves) with their
//! passkey; a passkey ceremony there opens a session bound to that package,
//! as a served frontend's is, under the origin's own rp-id; anyone else's
//! registration is refused; removing the origin ends those sessions.
//!
//! A sign-up on a registered origin is held to the cluster's sign-up policy
//! like any other.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

use calimero_client::Passkey;
use calimero_identity::testing::SoftAuthenticator;
use serde_json::json;

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, ops_signed, signed_up, Cluster, ORIGIN};

const NOTES: &str = "com.test.notes";
const OTHER: &str = "com.test.other";
/// The developer's own site for `NOTES`.
const SITE: &str = "https://notes.example.com";
const SITE_RP: &str = "notes.example.com";

/// A signed single-service `.mpk` of `package@version`, its WASM tagged so
/// each package ships its own.
fn release(package: &str, version: &str) -> Vec<u8> {
    use sha2::Digest;
    let key = ed25519_dalek::SigningKey::from_bytes(&[5; 32]);
    let mut wasm = kv_wasm();
    let tag = package.as_bytes();
    wasm.extend([
        0,
        u8::try_from(1 + tag.len()).unwrap(),
        u8::try_from(tag.len()).unwrap(),
    ]);
    wasm.extend_from_slice(tag);
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: package.to_owned(),
        app_version: version.to_owned(),
        signer_id: Some(calimero_bundle::derive_signer_id_did_key(
            key.verifying_key().as_bytes(),
        )),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: None,
        handlers: None,
        build_info: None,
        interfaces: None,
        wasm: Some(calimero_bundle::BundleArtifact {
            path: "app.wasm".to_owned(),
            hash: hex::encode(sha2::Sha256::digest(&wasm)),
            size: wasm.len() as u64,
        }),
        compute: None,
        abi: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        upgrades: None,
        outbound: Vec::new(),
        services: None,
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, &key).unwrap();
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("app.wasm", wasm),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

fn status(e: &calimero_client::Error) -> u16 {
    e.to_string()[..3].parse().unwrap()
}

/// Whether the node lets a browser page on `origin` call it (a CORS preflight).
async fn cors_allows(url: &str, origin: &str) -> bool {
    let r = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, format!("{url}auth/me"))
        .header("origin", origin)
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    r.headers()
        .get("access-control-allow-origin")
        .is_some_and(|v| v == origin)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registered_origin_signs_people_in_to_its_package_only() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let _ok = alice.upload_package(release(NOTES, "1.0.0")).await.unwrap();
    let _ok = bob.upload_package(release(OTHER, "1.0.0")).await.unwrap();
    let path = format!("package-origins/{NOTES}");

    // Before anything is registered, the site is nobody's.
    let carol = c.client(2);
    let e = carol
        .post(
            "auth/passkey/register/begin",
            json!({ "name": "carol", "origin": SITE }),
        )
        .await
        .unwrap_err();
    assert_eq!(
        (status(&e), e.code()),
        (403, Some("ORIGIN_NOT_REGISTERED")),
        "{e}"
    );
    // A ceremony made there anyway is not one the node takes.
    let begin = carol
        .post("auth/passkey/register/begin", json!({ "name": "carol" }))
        .await
        .unwrap();
    let mut carol_pk = SoftAuthenticator::new(SITE_RP, SITE);
    let e = carol
        .post(
            "auth/passkey/register/finish",
            json!({ "challengeId": begin["challengeId"], "registration": carol_pk.create(&begin["options"]) }),
        )
        .await
        .unwrap_err();
    assert_eq!(status(&e), 400, "an unregistered origin: {e}");
    assert!(!cors_allows(&c.urls[0], SITE).await);
    // The cluster's own origin, and a node's, begin as before, under the
    // cluster's rp-id.
    for origin in [
        ORIGIN.to_owned(),
        c.urls[1].trim_end_matches('/').to_owned(),
    ] {
        let begin = carol
            .post("auth/passkey/login/begin", json!({ "origin": origin }))
            .await
            .unwrap();
        assert_eq!(begin["options"]["rpId"], "localhost", "{origin}: {begin}");
    }

    // Bob is not the package's manager: refused, and his own valid passkey
    // consent over the change, logged without the API, is refused at apply.
    let e = bob
        .add_package_origin(NOTES, SITE, None, &mut bob_pk)
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "bob does not see the package: {e}");
    let statement = calimero_node::proof::Statement::new(
        &format!("package:{NOTES}"),
        "__add_origin",
        &json!({ "origin": SITE, "rpId": SITE_RP }),
    );
    let forged = json!({
        "op": "addPackageOrigin",
        "package": NOTES,
        "origin": SITE,
        "rp_id": SITE_RP,
        "actor": bob.me().await.unwrap()["accountId"],
        "at": 1,
        "consent": calimero_node::proof::Consent {
            assertion: bob_pk.assert(&statement.challenge().unwrap()),
            statement,
        },
    });
    let leader = c
        .nodes
        .iter()
        .flatten()
        .find(|r| r.node.system.status().is_leader)
        .unwrap();
    let (_index, out) = leader
        .node
        .system
        .write(serde_json::to_vec(&forged).unwrap())
        .await
        .unwrap();
    let refused = calimero_node::system::output(&out).unwrap_err();
    assert!(refused.contains("manager"), "{refused}");

    // What alice may not register.
    for (body, code) in [
        (
            json!({ "origin": "http://notes.example.com" }),
            "BAD_ORIGIN",
        ),
        (
            json!({ "origin": "https://notes.example.com/app" }),
            "BAD_ORIGIN",
        ),
        (json!({ "origin": "not an origin" }), "BAD_ORIGIN"),
        (
            json!({ "origin": SITE, "rpId": "example.org" }),
            "BAD_RP_ID",
        ),
        (json!({ "origin": ORIGIN }), "ORIGIN_TAKEN"),
    ] {
        let e = alice.post(&path, body.clone()).await.unwrap_err();
        assert_eq!(e.code(), Some(code), "{body}: {e}");
    }
    let e = alice
        .post(&path, json!({ "origin": SITE, "extra": 1 }))
        .await
        .unwrap_err();
    assert!(
        matches!(status(&e), 400 | 422),
        "unknown fields are refused: {e}"
    );

    // Alice registers her site with her passkey (412 first).
    let e = alice
        .post(&path, json!({ "origin": SITE }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let added = alice
        .add_package_origin(NOTES, SITE, None, &mut alice_pk)
        .await
        .unwrap();
    assert_eq!(
        (added["origin"].as_str(), added["rpId"].as_str()),
        (Some(SITE), Some(SITE_RP))
    );
    let listed = c.client(1);
    listed.use_session(alice.session().await.unwrap()).await;
    let list = listed.package_origins(NOTES).await.unwrap();
    assert_eq!(list["origins"][0]["origin"], SITE, "{list}");
    assert_eq!(list["origins"].as_array().unwrap().len(), 1);
    let e = bob.package_origins(NOTES).await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    // Bob's package cannot take alice's origin.
    let e = bob
        .add_package_origin(OTHER, SITE, None, &mut bob_pk)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("ORIGIN_TAKEN"), "{e}");
    assert!(
        cors_allows(&c.urls[2], SITE).await,
        "the site may call every node"
    );

    // Carol signs up on the site: a passkey for its rp-id, and a session
    // bound to NOTES that names the site.
    let begin = carol
        .post("auth/passkey/login/begin", json!({ "origin": SITE }))
        .await
        .unwrap();
    assert_eq!(begin["options"]["rpId"], SITE_RP, "{begin}");
    let s = carol
        .sign_up_at(&mut carol_pk, "carol", Some(SITE))
        .await
        .unwrap();
    let me = carol.me().await.unwrap();
    assert_eq!(
        (me["app"].as_str(), me["origin"].as_str()),
        (Some(NOTES), Some(SITE)),
        "{me}"
    );
    let claims = calimero_identity::peek_session(&s.session_token).unwrap();
    assert_eq!(
        (claims.app.as_deref(), claims.origin.as_deref()),
        (Some(NOTES), Some(SITE))
    );
    // What a package-bound session does: its own package's instances only,
    // no credentials, no other package.
    let notes_app = carol
        .create_app_from_package("notes", NOTES, None, None, json!({}))
        .await
        .unwrap();
    assert!(!notes_app.is_empty());
    let e = carol
        .create_app_from_package("other", OTHER, None, None, json!({}))
        .await
        .unwrap_err();
    assert!(matches!(status(&e), 403 | 404), "{e}");
    let e = carol
        .post("auth/apikeys", json!({ "label": "k" }))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let e = carol
        .post(&path, json!({ "origin": "https://x.example.com" }))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "a bound session registers nothing: {e}");
    // A step-up asked of that session names the site's rp-id, and her
    // passkey there answers it.
    let intents = format!("apps/{notes_app}/intents");
    let clear = json!({ "method": "clear", "args": {} });
    let e = carol.post(&intents, clear.clone()).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    assert_eq!(e.details().unwrap()["rpId"], SITE_RP, "{e}");
    let _ok = carol
        .post_confirmed(&intents, clear, &mut carol_pk)
        .await
        .unwrap();
    // Her session refreshes on any node.
    let other_node = c.client(0);
    other_node.use_session(carol.session().await.unwrap()).await;
    let _s = other_node.refresh().await.unwrap();
    assert_eq!(other_node.me().await.unwrap()["origin"], SITE);

    // Alice's cluster passkey does not sign in on the site (another rp-id).
    let alice_there = c.client(1);
    let mut alice_on_site = SoftAuthenticator::with_key(&alice_pk, "localhost", SITE);
    let e = alice_there
        .sign_in_at(&mut alice_on_site, Some(SITE))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
    // Carol signs in again there.
    let again = c.client(1);
    let _s = again.sign_in_at(&mut carol_pk, Some(SITE)).await.unwrap();

    // Removal: the sessions opened there end, their refresh with them, and
    // the site may not begin a ceremony or call the node any more.
    let removed = alice
        .remove_package_origin(NOTES, SITE, &mut alice_pk)
        .await
        .unwrap();
    assert_eq!(removed["removed"], true);
    for client in [&carol, &other_node] {
        let e = client.me().await.unwrap_err();
        assert_eq!(status(&e), 401, "{e}");
        let e = client.refresh().await.unwrap_err();
        assert_eq!(status(&e), 401, "{e}");
    }
    let e = c
        .client(2)
        .sign_in_at(&mut carol_pk, Some(SITE))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("ORIGIN_NOT_REGISTERED"), "{e}");
    assert!(!cors_allows(&c.urls[0], SITE).await);
    assert!(alice.package_origins(NOTES).await.unwrap()["origins"]
        .as_array()
        .unwrap()
        .is_empty());
}

/// A sign-up on a registered origin is judged by the cluster's sign-up
/// policy like any other: on an invite-only cluster it is refused without an
/// invite, and taken with one, bound to the origin's package.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registered_origin_signs_up_only_whom_the_cluster_takes() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance.sign_up = calimero_node::system::SignUp::Invite;
    })
    .await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let expires = chrono::Utc::now() + chrono::Duration::hours(1);
    for token in ["sup_alice_000000000", "sup_carol_000000000"] {
        let _i = ops_signed(
            &ops,
            "ops/sign-up/invites",
            json!({ "token": token, "uses": 1, "expiresAt": expires.to_rfc3339() }),
        )
        .await
        .unwrap();
    }
    let alice = c.client(0);
    let mut alice_pk = SoftAuthenticator::new("localhost", ORIGIN);
    let _s = alice
        .sign_up_invited(&mut alice_pk, "alice", Some("sup_alice_000000000"))
        .await
        .unwrap();
    let _ok = alice.upload_package(release(NOTES, "1.0.0")).await.unwrap();
    let _ok = alice
        .add_package_origin(NOTES, SITE, None, &mut alice_pk)
        .await
        .unwrap();

    // No invite: refused at begin on the site, before any passkey is made.
    let carol = c.client(1);
    let mut carol_pk = SoftAuthenticator::new(SITE_RP, SITE);
    let e = carol
        .sign_up_with(&mut carol_pk, "carol", Some(SITE), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("SIGN_UP_REFUSED"), "{e}");
    // With one: an account whose session is bound to the site's package.
    let _s = carol
        .sign_up_with(
            &mut carol_pk,
            "carol",
            Some(SITE),
            Some("sup_carol_000000000"),
        )
        .await
        .unwrap();
    let me = carol.me().await.unwrap();
    assert_eq!(
        (me["app"].as_str(), me["origin"].as_str()),
        (Some(NOTES), Some(SITE)),
        "{me}"
    );
}
