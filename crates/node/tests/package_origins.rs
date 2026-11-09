//! Registered package origins: a package's manager registers a web origin
//! of their own (a developer's site, not one the node serves) with their
//! passkey; a passkey ceremony there opens a session bound to that package,
//! as a served frontend's is, under the origin's own rp-id; anyone else's
//! registration is refused; removing the origin ends those sessions.
//!
//! A sign-up on a registered origin is held to the cluster's sign-up policy
//! like any other.
//!
//! A served origin (a custom domain, `served: true`) is one the cluster also
//! serves the package's frontend on, from its signed bundle; everything else
//! about it is a registered origin's.
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
    release_with(package, version, &[])
}

/// [`release`] shipping `frontend` files (paths under `frontend/`, the first
/// is the entry).
fn release_with(package: &str, version: &str, frontend: &[(&str, &[u8])]) -> Vec<u8> {
    use sha2::Digest;
    let artifact = |path: &str, bytes: &[u8]| calimero_bundle::BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
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
        wasm: Some(artifact("app.wasm", wasm.as_slice())),
        compute: None,
        abi: None,
        links: None,
        frontend: (!frontend.is_empty()).then(|| calimero_bundle::BundleFrontend {
            entry: frontend[0].0.to_owned(),
            files: frontend.iter().map(|(p, b)| artifact(p, b)).collect(),
        }),
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
    let mut entries = vec![
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("app.wasm", wasm),
    ];
    entries.extend(frontend.iter().map(|(p, b)| (*p, b.to_vec())));
    for (name, bytes) in entries {
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
    // Her password manager lists the passkey under the app and the cluster
    // (the package id: its release names no display name).
    let begin = carol
        .post(
            "auth/passkey/register/begin",
            json!({ "name": "carol", "origin": SITE }),
        )
        .await
        .unwrap();
    let public = c.client(0).get("sign-up").await.unwrap();
    let tag = public["passkeyTag"].as_str().unwrap().to_owned();
    assert_eq!(
        begin["options"]["user"]["name"],
        format!("carol · {NOTES} ({} · {tag})", c.name),
        "{begin}"
    );
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

/// A package's manager has the cluster serve its frontend on a domain of
/// their own: registered `served: true` with their passkey, the node answers
/// that host with the package's site from its signed bundle (and nothing of
/// its own routes), CORS admits it, a passkey ceremony there opens a session
/// bound to the package under the domain's own rp-id, the operator sees it in
/// `/ops/served-origins` for its certificate and route, and removing it ends
/// all of that. A plain registered origin is never served; a served one with
/// a port, under the apps domain or on a node's address is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_served_origin_serves_its_packages_frontend_and_binds_its_sessions() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("apps.localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let html: &[u8] = b"<!doctype html><html><body>notes</body></html>";
    let _ok = alice
        .upload_package(release_with(
            NOTES,
            "1.0.0",
            &[
                ("frontend/index.html", html),
                ("frontend/assets/app.js", b"console.log(1)"),
            ],
        ))
        .await
        .unwrap();
    let _ok = bob.upload_package(release(OTHER, "1.0.0")).await.unwrap();
    let path = format!("package-origins/{NOTES}");

    // `merod local`'s stand-in for `https://kv.example.com`: a localhost name,
    // with node 0's port (a custom domain in production has none).
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let host = "kv.localhost";
    let origin = format!("http://{host}:{port}");
    let rp = host;
    let web = reqwest::Client::builder()
        .resolve(host, ([127, 0, 0, 1], port).into())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let get = |p: &str| {
        web.get(format!("{origin}{p}"))
            .header("accept", "text/html")
            .send()
    };
    let well_known = |known: reqwest::Response| async move {
        let ok = known.status() == 200;
        let body: serde_json::Value = known.json().await.unwrap_or_default();
        ok.then_some(body)
    };

    // Not yet registered: the host is no site.
    assert!(
        well_known(get("/.well-known/calimero.json").await.unwrap())
            .await
            .is_none_or(|b| b["package"] != NOTES),
        "an unregistered host serves no package"
    );

    // What may not be served.
    let apps_host = format!("http://x.apps.localhost:{port}");
    let node_addr = format!("http://127.0.0.1:{port}");
    for (origin, code) in [
        ("http://kv.localhost", "BAD_ORIGIN"),
        (apps_host.as_str(), "BAD_ORIGIN"),
        (node_addr.as_str(), "BAD_ORIGIN"),
    ] {
        let e = alice
            .post(&path, json!({ "origin": origin, "served": true }))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some(code), "{origin}: {e}");
    }
    let e = bob
        .post_confirmed(
            &path,
            json!({ "origin": origin, "served": true }),
            &mut bob_pk,
        )
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "bob does not manage the package: {e}");

    // A plain registration is never served.
    let plain = format!("http://plain.localhost:{port}");
    let _ok = alice
        .add_package_origin(NOTES, &plain, None, &mut alice_pk)
        .await
        .unwrap();
    let plain_web = reqwest::Client::builder()
        .resolve("plain.localhost", ([127, 0, 0, 1], port).into())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let r = plain_web
        .get(format!("{plain}/.well-known/calimero.json"))
        .send()
        .await
        .unwrap();
    assert!(
        well_known(r).await.is_none_or(|b| b["package"] != NOTES),
        "a plain registered origin is the developer's to host"
    );
    // ...nor made served by a second registration of it.
    let e = alice
        .post(&path, json!({ "origin": plain, "served": true }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("ORIGIN_TAKEN"), "{e}");

    // Alice registers the domain served, with her passkey (412 first).
    let body = json!({ "origin": origin, "served": true });
    let e = alice.post(&path, body.clone()).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let added = alice
        .post_confirmed(&path, body, &mut alice_pk)
        .await
        .unwrap();
    assert_eq!(
        (
            added["origin"].as_str(),
            added["rpId"].as_str(),
            added["served"].as_bool()
        ),
        (Some(origin.as_str()), Some(rp), Some(true)),
        "{added}"
    );
    let e = bob
        .post_confirmed(
            &format!("package-origins/{OTHER}"),
            json!({ "origin": origin, "served": true }),
            &mut bob_pk,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("ORIGIN_TAKEN"), "{e}");

    // The node serves the package's site there, and nothing of its own.
    let root = get("/").await.unwrap();
    assert_eq!(root.status(), 302);
    assert_eq!(root.headers()["location"], "/1.0.0/");
    let page = get("/1.0.0/").await.unwrap();
    assert_eq!(page.status(), 200);
    let csp = page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    assert!(csp.contains("default-src 'self'"), "{csp}");
    assert!(
        csp.contains("https://cloud.calimero.network"),
        "the attestation mirror is reachable: {csp}"
    );
    assert_eq!(page.bytes().await.unwrap().as_ref(), html);
    assert_eq!(get("/1.0.0/docs/42").await.unwrap().status(), 200);
    assert_eq!(get("/health").await.unwrap().status(), 404);
    let known = well_known(get("/.well-known/calimero.json").await.unwrap())
        .await
        .expect("the site's well-known file");
    assert_eq!(known["package"], NOTES, "{known}");
    assert!(
        cors_allows(&c.urls[2], &origin).await,
        "every node admits it"
    );

    // The operator sees it, by the package's label, never its name.
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let listed = ops.get("ops/served-origins").await.unwrap();
    let served = listed["origins"].as_array().unwrap();
    assert_eq!(served.len(), 1, "the plain origin is not listed: {listed}");
    assert_eq!(served[0]["origin"], origin.as_str());
    assert_eq!(served[0]["host"], host);
    assert_eq!(
        served[0]["label"],
        calimero_node::app_host::label(NOTES).as_str()
    );
    assert!(served[0].get("package").is_none(), "{listed}");
    assert!(alice.get("ops/served-origins").await.is_err());

    // Carol signs up on the domain: a passkey for its own rp-id, a session
    // bound to the package that names the domain.
    let carol = c.client(2);
    let begin = carol
        .post("auth/passkey/login/begin", json!({ "origin": origin }))
        .await
        .unwrap();
    assert_eq!(begin["options"]["rpId"], rp, "{begin}");
    let mut carol_pk = SoftAuthenticator::new(rp, &origin);
    let _s = carol
        .sign_up_at(&mut carol_pk, "carol", Some(&origin))
        .await
        .unwrap();
    let me = carol.me().await.unwrap();
    assert_eq!(
        (me["app"].as_str(), me["origin"].as_str()),
        (Some(NOTES), Some(origin.as_str())),
        "{me}"
    );
    let e = carol
        .create_app_from_package("other", OTHER, None, None, json!({}))
        .await
        .unwrap_err();
    assert!(matches!(status(&e), 403 | 404), "{e}");
    // Alice's cluster passkey does not answer there.
    let mut alice_there = SoftAuthenticator::with_key(&alice_pk, "localhost", &origin);
    let e = c
        .client(1)
        .sign_in_at(&mut alice_there, Some(&origin))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 401, "{e}");

    // Removal: the site goes, and so do its sessions and its CORS.
    let removed = alice
        .remove_package_origin(NOTES, &origin, &mut alice_pk)
        .await
        .unwrap();
    assert_eq!(removed["removed"], true);
    assert!(
        well_known(get("/.well-known/calimero.json").await.unwrap())
            .await
            .is_none_or(|b| b["package"] != NOTES),
        "a removed domain serves nothing"
    );
    assert_ne!(get("/").await.unwrap().status(), 302);
    let e = carol.me().await.unwrap_err();
    assert_eq!(status(&e), 401, "{e}");
    assert!(!cors_allows(&c.urls[0], &origin).await);
    assert!(ops.get("ops/served-origins").await.unwrap()["origins"]
        .as_array()
        .unwrap()
        .is_empty());
}
