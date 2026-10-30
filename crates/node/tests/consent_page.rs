//! The node's consent page: an app asks its user to approve something only
//! the account holder may approve (an upgrade, a mero-tee release, outbound
//! prefixes), the node serves a page of its own on its own origin showing
//! node-side facts only, and the person's passkey assertion there is the
//! consent, bound to exactly what the page showed.
//!
//! Needs `kv_store.wasm` (see `cluster.rs`).

use calimero_identity::testing::SoftAuthenticator;
use calimero_node::api::Pending;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, Cluster, ORIGIN};

const PACKAGE: &str = "com.test.notes";

/// What a release declares beyond its code.
#[derive(Default)]
struct Declares<'a> {
    outbound: &'a [&'a str],
    frontend: Option<&'a str>,
    aliases: &'a [&'a str],
}

/// A signed single-service `.mpk` of `PACKAGE`, and its manifest's hash.
fn release(
    version: &str,
    wasm: &[u8],
    key: &ed25519_dalek::SigningKey,
    d: &Declares<'_>,
) -> (Vec<u8>, String) {
    let sha = |b: &[u8]| hex::encode(calimero_identity::sha256(b));
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: PACKAGE.to_owned(),
        app_version: version.to_owned(),
        signer_id: Some(calimero_bundle::derive_signer_id_did_key(
            key.verifying_key().as_bytes(),
        )),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: Some(calimero_bundle::BundleMetadata {
            name: "Notes".to_owned(),
            description: None,
            author: Some("Example".to_owned()),
            icon: None,
            tags: Vec::new(),
            license: None,
            category: None,
            guide: None,
        }),
        handlers: None,
        build_info: None,
        interfaces: None,
        wasm: Some(calimero_bundle::BundleArtifact {
            path: "app.wasm".to_owned(),
            hash: sha(wasm),
            size: wasm.len() as u64,
        }),
        abi: None,
        links: d.frontend.map(|f| calimero_bundle::BundleLinks {
            frontend: Some(f.to_owned()),
            github: None,
            docs: None,
        }),
        frontend: None,
        frontend_aliases: d.aliases.iter().map(|a| (*a).to_owned()).collect(),
        outbound: d.outbound.iter().map(|u| (*u).to_owned()).collect(),
        services: None,
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, key).unwrap();
    let hash = hex::encode(calimero_bundle::compute_bundle_hash(
        &calimero_bundle::canonicalize_manifest(&manifest).unwrap(),
    ));
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in [
        ("manifest.json", serde_json::to_vec(&manifest).unwrap()),
        ("app.wasm", wasm.to_vec()),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    (tar.into_inner().unwrap().finish().unwrap(), hash)
}

/// `wasm` with a custom section named `tag`: another module that runs the same.
fn variant(wasm: &[u8], tag: &str) -> Vec<u8> {
    let mut out = wasm.to_vec();
    out.push(0);
    out.push(u8::try_from(1 + tag.len()).unwrap());
    out.push(u8::try_from(tag.len()).unwrap());
    out.extend_from_slice(tag.as_bytes());
    out
}

fn status(e: &calimero_client::Error) -> u16 {
    e.to_string()[..3].parse().unwrap()
}

/// The page's origin, from the url a request answered.
fn origin(url: &str) -> String {
    url::Url::parse(url).unwrap().origin().ascii_serialization()
}

/// The arguments the node's pending challenge `id` commits to.
fn challenge_args(c: &Cluster, id: &str) -> Value {
    for r in c.nodes.iter().flatten() {
        if let Some(Pending::Confirm { args, method, .. }) =
            r.node.challenges.lock().unwrap().get(id)
        {
            let mut v: Value = serde_json::from_slice(args).unwrap();
            v["__method"] = json!(method);
            return v;
        }
    }
    panic!("no pending challenge {id}");
}

/// A browser on the page: plain HTTP to the node's own origin, no session.
struct Page {
    http: reqwest::Client,
    url: String,
}

impl Page {
    fn new(url: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            url: url.to_owned(),
        }
    }

    async fn model(&self) -> Value {
        let r = self
            .http
            .get(format!("{}/model", self.url))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        r.json().await.unwrap()
    }

    async fn post(&self, what: &str, body: Value) -> (u16, Value) {
        let r = self
            .http
            .post(format!("{}/{what}", self.url))
            .header("origin", origin(&self.url))
            .json(&body)
            .send()
            .await
            .unwrap();
        (r.status().as_u16(), r.json().await.unwrap_or(Value::Null))
    }

    /// Approve with `key` (the person's passkey on the page's origin): the
    /// 412, then the assertion over its challenge. Answers the second reply
    /// and the arguments the challenge committed to.
    async fn approve(&self, c: &Cluster, key: &mut SoftAuthenticator) -> ((u16, Value), Value) {
        let (code, asked) = self.post("approve", json!({})).await;
        assert_eq!(code, 412, "{asked}");
        assert_eq!(asked["error"], "CONFIRM_REQUIRED");
        let d = &asked["details"];
        let id = d["challengeId"].as_str().unwrap();
        let args = challenge_args(c, id);
        let challenge = calimero_identity::from_b64url(d["challenge"].as_str().unwrap()).unwrap();
        let assertion = key.assert(&challenge);
        let answer = self
            .post(
                "approve",
                json!({ "confirm": { "challengeId": id, "assertion": assertion } }),
            )
            .await;
        (answer, args)
    }
}

/// Every value at `paths` (JSON pointers) replaced, for a stable snapshot.
fn redact(mut v: Value, paths: &[&str]) -> Value {
    for p in paths {
        if let Some(x) = v.pointer_mut(p) {
            *x = json!("<redacted>");
        }
    }
    v
}

/// An app's own frontend asks for an upgrade; the node's page shows the
/// capability sheet from the two signed manifests, the person approves with
/// their passkey there, and the consent commits to the target's manifest.
/// The app's package-bound session still cannot approve directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_app_asks_and_the_person_approves_on_the_nodes_page() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let (v1, v1_hash) = release(
        "1.0.0",
        &kv_wasm(),
        &publisher,
        &Declares {
            outbound: &["https://api.example/v1/"],
            frontend: Some("https://notes.example"),
            ..Declares::default()
        },
    );
    let (v2, v2_hash) = release(
        "2.0.0",
        &variant(&kv_wasm(), "v2"),
        &publisher,
        &Declares {
            outbound: &["https://api.example/v1/", "https://metrics.example/"],
            frontend: Some("https://notes.example"),
            aliases: &["https://share.example/notes"],
        },
    );
    let _ = alice.upload_package(v1).await.unwrap();
    let _ = alice.upload_package(v2).await.unwrap();
    let app = alice
        .post(
            "apps",
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();

    // Alice in the app's own frontend: a session bound to the package.
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(1);
    let mut in_app_passkey = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut in_app_passkey).await.unwrap();

    // It may not approve anything itself: the routes refuse it, ceremony or not.
    for (path, body) in [
        (format!("apps/{app}/upgrade"), json!({ "version": "2.0.0" })),
        ("auth/releases".to_owned(), json!({ "version": "9.9.9" })),
        (
            "auth/outbound".to_owned(),
            json!({ "package": PACKAGE, "prefixes": ["https://metrics.example/"] }),
        ),
    ] {
        let e = in_app.post(&path, body).await.unwrap_err();
        assert_eq!(status(&e), 403, "{path}: {e}");
    }

    // It asks the node instead, and may only be sent back to its own origin.
    let ask = |return_to: &str| {
        json!({
            "kind": "upgrade",
            "subject": { "app": app, "toVersion": "2.0.0" },
            "returnTo": return_to,
        })
    };
    for elsewhere in [
        "https://evil.example/".to_owned(),
        format!("{ORIGIN}/back"),
        "javascript:alert(1)".to_owned(),
        format!("{app_origin}/back#frag"),
    ] {
        let e = in_app
            .post("consent/requests", ask(&elsewhere))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("RETURN_TO_NOT_ALLOWED"), "{elsewhere}: {e}");
    }
    let mut unknown = ask(&format!("{app_origin}/back"));
    unknown["extra"] = json!(1);
    let e = in_app.post("consent/requests", unknown).await.unwrap_err();
    assert_eq!(status(&e), 400, "unknown fields are refused: {e}");
    let made = in_app
        .post(
            "consent/requests",
            ask(&format!("{app_origin}/1.0.0/?tab=2")),
        )
        .await
        .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let url = made["url"].as_str().unwrap().to_owned();
    assert_eq!(
        url,
        format!("{}consent/{id}", c.urls[1]),
        "the node's own origin"
    );

    // The page: the node's static bytes under a strict policy, no app data.
    let page = Page::new(&url);
    let html = page.http.get(&url).send().await.unwrap();
    assert_eq!(html.status(), 200);
    let csp = html.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .to_owned();
    for want in [
        "default-src 'none'",
        "frame-ancestors 'none'",
        "connect-src 'self'",
    ] {
        assert!(csp.contains(want), "{csp}");
    }
    assert!(!csp.contains("unsafe-inline"), "{csp}");
    assert_eq!(html.headers()["x-frame-options"], "DENY");
    let body = html.text().await.unwrap();
    assert!(
        !body.contains(PACKAGE) && !body.contains(&app),
        "static, no data"
    );
    // Only the node's own origins may post a decision.
    let r = page
        .http
        .post(format!("{url}/decline"))
        .header("origin", app_origin.as_str())
        .send()
        .await
        .unwrap();
    assert_eq!(
        r.status(),
        403,
        "an app's script cannot decline for the person"
    );

    // What the page renders, from node-side facts only.
    let model = page.model().await;
    let snapshot = redact(
        model.clone(),
        &[
            "/requestId",
            "/expiresAt",
            "/approver/account",
            "/scope/app",
            "/after",
            "/upgrade/from/bundle",
            "/upgrade/from/mpk",
            "/upgrade/from/signer",
            "/upgrade/to/bundle",
            "/upgrade/to/mpk",
            "/upgrade/to/signer",
            "/upgrade/publisher/signer",
        ],
    );
    let expected = json!({
        "kind": "upgrade",
        "status": "pending",
        "decidedAt": null,
        "rpId": "localhost",
        "requestId": "<redacted>",
        "expiresAt": "<redacted>",
        "approver": { "account": "<redacted>" },
        "requestedBy": { "package": PACKAGE, "returnOrigin": app_origin },
        "scope": { "kind": "app", "app": "<redacted>", "name": "notes", "org": null },
        "headline": "Upgrade Notes from 1.0.0 to 2.0.0",
        "before": "Until you approve, the app keeps running 1.0.0: only that code reads and changes its data.",
        "after": "<redacted>",
        "notes": [
            "2.0.0 names 1 new web address(es) its sign-in may return to: https://share.example. Pages there could act in this app as you.",
            "2.0.0 declares 1 new address(es) it would send data to: https://metrics.example/. It cannot reach them until they are approved too.",
        ],
        "upgrade": {
            "from": {
                "package": PACKAGE, "version": "1.0.0", "service": null, "name": "Notes",
                "bundle": "<redacted>", "mpk": "<redacted>", "manifestHash": v1_hash, "signer": "<redacted>",
            },
            "to": {
                "package": PACKAGE, "version": "2.0.0", "service": null, "name": "Notes",
                "bundle": "<redacted>", "mpk": "<redacted>", "manifestHash": v2_hash, "signer": "<redacted>",
            },
            "publisher": { "signer": "<redacted>", "author": "Example", "changed": false },
            "outbound": {
                "added": ["https://metrics.example/"],
                "removed": [],
                "kept": ["https://api.example/v1/"],
            },
            "frontend": {
                "from": "https://notes.example",
                "to": "https://notes.example",
                "changed": false,
                "aliases": { "added": ["https://share.example/notes"], "removed": [], "kept": [] },
                "newLoginOrigins": ["https://share.example"],
                "droppedLoginOrigins": [],
                "ships": { "from": false, "to": false },
            },
            "interfaces": {
                "uses": { "added": [], "removed": [], "kept": [] },
                "exports": { "added": [], "removed": [], "kept": [] },
            },
            "abi": { "from": model["upgrade"]["abi"]["from"], "to": model["upgrade"]["abi"]["to"] },
            "methods": model["upgrade"]["methods"],
            "events": model["upgrade"]["events"],
            "timers": { "added": [], "removed": [], "changed": [] },
            "migration": model["upgrade"]["migration"],
            "buildInfo": { "from": null, "to": null, "changed": false },
            "minRuntimeVersion": { "from": "0.1.0", "to": "0.1.0" },
        },
    });
    assert_eq!(
        snapshot,
        expected,
        "{}",
        serde_json::to_string_pretty(&snapshot).unwrap()
    );
    assert_eq!(
        model["upgrade"]["migration"]["runs"], false,
        "same state version"
    );

    // Approved on the page: the passkey signs the target's manifest hash.
    // The same passkey (one signature counter) on the node's origin.
    let mut page_key = SoftAuthenticator::with_key(&in_app_passkey, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut page_key).await;
    assert_eq!(args["__method"], "__upgrade");
    assert_eq!(args["manifest"], json!(v2_hash), "{args}");
    assert_eq!(code, 200, "{done}");
    assert_eq!(
        done["redirect"],
        json!(format!(
            "{app_origin}/1.0.0/?tab=2&request_id={id}&result=approved"
        ))
    );
    let rec = alice.get(&format!("apps/{app}")).await.unwrap();
    let v2_bundle = model["upgrade"]["to"]["bundle"].as_str().unwrap();
    assert_eq!(rec["bundle"], json!(v2_bundle), "{rec}");

    // Decided once; the app reads the result; nobody else does.
    let (code, again) = page.post("approve", json!({})).await;
    assert_eq!(
        (code, again["error"].as_str()),
        (409, Some("ALREADY_DECIDED"))
    );
    let (code, _) = page.post("decline", json!({})).await;
    assert_eq!(code, 409);
    let result = in_app.get(&format!("consent/{id}/result")).await.unwrap();
    assert_eq!(result["status"], "approved");
    let on_other_node = c.client(0);
    on_other_node
        .use_session(in_app.session().await.unwrap())
        .await;
    let result = on_other_node
        .get(&format!("consent/{id}/result"))
        .await
        .unwrap();
    assert_eq!(result["status"], "approved", "asked through another node");
    let (bob, _) = signed_up(&c, 2, "bob").await;
    let e = bob.get(&format!("consent/{id}/result")).await.unwrap_err();
    assert_eq!(status(&e), 404);
}

/// A bundle re-installed under the version the person was shown is not the
/// one they approve: the page refuses it as stale, and the route refuses an
/// upgrade whose target manifest is not the one named.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_bundle_swapped_under_the_same_version_is_refused() {
    let c = Cluster::start(3).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[10; 32]);
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let wasm2 = variant(&kv_wasm(), "v2");
    let (shown, shown_hash) = release(
        "2.0.0",
        &wasm2,
        &publisher,
        &Declares {
            outbound: &["https://api.example/"],
            ..Declares::default()
        },
    );
    let (swapped, swapped_hash) = release(
        "2.0.0",
        &wasm2,
        &publisher,
        &Declares {
            outbound: &["https://api.example/", "https://exfiltrate.example/"],
            ..Declares::default()
        },
    );
    assert_ne!(shown_hash, swapped_hash);
    let _ = alice.upload_package(v1).await.unwrap();
    let _ = alice.upload_package(shown).await.unwrap();
    let app = alice
        .post(
            "apps",
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let made = alice
        .post(
            "consent/requests",
            json!({
                "kind": "upgrade",
                "subject": { "app": app, "toVersion": "2.0.0" },
                "returnTo": format!("{ORIGIN}/done"),
            }),
        )
        .await
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    assert_eq!(
        page.model().await["upgrade"]["to"]["manifestHash"],
        json!(shown_hash)
    );

    // The same WASM under the same version, another manifest.
    let _ = alice
        .delete(&format!("packages/{PACKAGE}/2.0.0"))
        .await
        .unwrap();
    let _ = alice.upload_package(swapped).await.unwrap();

    let (code, refused) = page.post("approve", json!({})).await;
    assert_eq!(
        (code, refused["error"].as_str()),
        (409, Some("CONSENT_STALE")),
        "{refused}"
    );
    // The route binds whatever manifest the target has now: the swapped one.
    let e = alice
        .post(
            &format!("apps/{app}/upgrade"),
            json!({ "version": "2.0.0" }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let id = e.details().unwrap()["challengeId"].as_str().unwrap();
    assert_eq!(challenge_args(&c, id)["manifest"], json!(swapped_hash));
    let mut key = SoftAuthenticator::with_key(&alice_key, "localhost", ORIGIN);
    let _ok = alice
        .post_confirmed(
            &format!("apps/{app}/upgrade"),
            json!({ "version": "2.0.0" }),
            &mut key,
        )
        .await
        .unwrap();
}

/// Release and outbound requests through the page, a person's own account
/// as the scope; the bounds a request is held to.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn releases_and_outbound_prefixes_are_approved_on_the_page_within_bounds() {
    let c = Cluster::start(3).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    let (v1, _) = release(
        "1.0.0",
        &kv_wasm(),
        &publisher,
        &Declares {
            outbound: &["https://api.example/", "https://other.example/"],
            ..Declares::default()
        },
    );
    let _ = alice.upload_package(v1).await.unwrap();
    let back = format!("{ORIGIN}/settings");

    // A release: the model says what changes before and after.
    let made = alice
        .post(
            "consent/requests",
            json!({
                "kind": "release",
                "scope": "account",
                "subject": { "version": "2.0.0" },
                "returnTo": back,
            }),
        )
        .await
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["kind"], "release");
    assert!(
        model["before"].as_str().unwrap().starts_with(
            "Until you approve, nodes running mero-tee 2.0.0 receive none of the data"
        ),
        "{model}"
    );
    assert_eq!(model["release"]["alreadyApproved"], false);
    // Mock attestation: no node checks measurements; the digest is over none.
    let measurements = &model["release"]["measurements"];
    assert_eq!(measurements["profiles"], json!({}));
    assert!(measurements["mock"].is_string(), "{measurements}");
    let digest = hex::encode(calimero_identity::sha256(
        json!({ "profiles": {}, "version": "2.0.0" })
            .to_string()
            .as_bytes(),
    ));
    assert_eq!(measurements["policyDigest"], json!(digest));
    // One passkey, one signature counter, on node 1's origin.
    let mut page_key = SoftAuthenticator::with_key(&alice_key, "localhost", &origin(&c.urls[0]));
    let ((code, done), args) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(
        args,
        json!({ "version": "2.0.0", "policyDigest": digest, "__method": "__approve_release" })
    );
    let me = alice.me().await.unwrap();
    assert!(
        me["approvedReleases"]
            .as_array()
            .unwrap()
            .contains(&json!("2.0.0")),
        "{me}"
    );

    // Outbound: exactly the prefixes listed, never "whatever is declared".
    let made = alice
        .post(
            "consent/requests",
            json!({
                "kind": "outbound",
                "scope": "account",
                "subject": { "package": PACKAGE, "prefixes": ["https://api.example/"] },
                "returnTo": back,
            }),
        )
        .await
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["outbound"]["new"], json!(["https://api.example/"]));
    let ((code, done), args) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["prefixes"], json!(["https://api.example/"]));
    let e = alice
        .post(
            "consent/requests",
            json!({
                "kind": "outbound",
                "scope": "account",
                "subject": { "package": PACKAGE, "prefixes": ["https://undeclared.example/"] },
                "returnTo": back,
            }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_DECLARED"), "{e}");

    // Declined: the app is sent back with the result, and nothing is approved.
    let made = alice
        .post(
            "consent/requests",
            json!({
                "kind": "release",
                "scope": "account",
                "subject": { "version": "3.0.0" },
                "returnTo": back,
            }),
        )
        .await
        .unwrap();
    let id = made["requestId"].as_str().unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let (code, declined) = page.post("decline", json!({})).await;
    assert_eq!(code, 200);
    assert_eq!(
        declined["redirect"],
        json!(format!("{back}?request_id={id}&result=declined"))
    );
    let result = alice.get(&format!("consent/{id}/result")).await.unwrap();
    assert_eq!(result["status"], "declined");

    // At most this many undecided requests per account.
    let ask = json!({
        "kind": "release",
        "scope": "account",
        "subject": { "version": "4.0.0" },
        "returnTo": back,
    });
    for _ in 0..calimero_node::consent_page::MAX_OPEN_REQUESTS_PER_ACCOUNT {
        let _ = alice.post("consent/requests", ask.clone()).await.unwrap();
    }
    let e = alice.post("consent/requests", ask).await.unwrap_err();
    assert_eq!(e.code(), Some("RATE_LIMITED"), "{e}");

    // An id this node never issued has no page.
    let r = reqwest::get(format!(
        "{}consent/1.AAAAAAAAAAAAAAAAAAAAAA/model",
        c.urls[0]
    ))
    .await
    .unwrap();
    assert_eq!(r.status(), 404);
}

/// A console asks the page to withdraw a release approval: the page says what
/// stops and what stays, shows no measurements (withdrawing never waits on
/// fetching them), and the consent binds the version alone. The node still
/// refuses to withdraw the last approved release.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_release_approval_is_withdrawn_on_the_page() {
    let c = Cluster::start(3).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let back = format!("{ORIGIN}/settings");
    let ask = |version: &str, withdraw: bool| {
        json!({
            "kind": "release",
            "scope": "account",
            "subject": { "version": version, "withdraw": withdraw },
            "returnTo": back,
        })
    };
    let approved = || async {
        alice.me().await.unwrap()["approvedReleases"]
            .as_array()
            .unwrap()
            .clone()
    };
    let first = approved().await;
    assert_eq!(first.len(), 1, "the sign-up node's release: {first:?}");
    let first = first[0].as_str().unwrap().to_owned();
    let mut page_key = SoftAuthenticator::with_key(&alice_key, "localhost", &origin(&c.urls[0]));

    // A second release, approved on the page.
    let made = alice
        .post("consent/requests", ask("2.0.0", false))
        .await
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let ((code, done), _) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    assert!(approved().await.contains(&json!("2.0.0")));

    // Withdrawn on the page: no measurements, the version alone is bound.
    let made = alice
        .post("consent/requests", ask("2.0.0", true))
        .await
        .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["kind"], "release");
    assert_eq!(model["release"]["change"], "withdraw", "{model}");
    assert!(
        model["headline"]
            .as_str()
            .unwrap()
            .starts_with("Stop nodes running mero-tee 2.0.0 from receiving the data of"),
        "{model}"
    );
    assert!(model["release"].get("measurements").is_none(), "{model}");
    assert_eq!(model["release"]["currentlyApproved"], true);
    assert_eq!(model["release"]["lastApproved"], false);
    let ((code, done), args) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(
        args,
        json!({ "version": "2.0.0", "__method": "__withdraw_release" })
    );
    assert_eq!(
        done["redirect"],
        json!(format!("{back}?request_id={id}&result=approved"))
    );
    assert_eq!(approved().await, vec![json!(first)]);

    // A release that is not approved: the page says withdrawing changes nothing.
    let made = alice
        .post("consent/requests", ask("5.0.0", true))
        .await
        .unwrap();
    let model = Page::new(made["url"].as_str().unwrap()).model().await;
    assert_eq!(model["release"]["currentlyApproved"], false, "{model}");
    assert!(
        model["notes"][0]
            .as_str()
            .unwrap()
            .contains("is not approved"),
        "{model}"
    );

    // The last approved release: the page warns, and the node refuses it.
    let made = alice
        .post("consent/requests", ask(&first, true))
        .await
        .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["release"]["lastApproved"], true, "{model}");
    let ((code, refused), _) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 400, "{refused}");
    assert_eq!(refused["error"], "REJECTED");
    assert_eq!(
        refused["message"],
        "the last approved release cannot be withdrawn"
    );
    assert_eq!(approved().await, vec![json!(first)]);
    let result = alice.get(&format!("consent/{id}/result")).await.unwrap();
    assert_eq!(result["status"], "pending");

    // The subject is still closed.
    let mut unknown = ask("2.0.0", true);
    unknown["subject"]["approved"] = json!(false);
    let e = alice.post("consent/requests", unknown).await.unwrap_err();
    assert_eq!(status(&e), 400, "{e}");
}
