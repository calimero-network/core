//! The node's consent page: an app asks its user to approve something only
//! the account holder may approve (an upgrade, a mero-tee release, outbound
//! prefixes), the node serves a page of its own on its own origin showing
//! node-side facts only, and the person's passkey assertion there is the
//! consent, bound to exactly what the page showed.
//!
//! Builds the component fixtures `apps/fixture-kv`, `notes-v1` and `notes-v2`
//! from this checkout (see `harness/cluster.rs`).

use calimero_identity::testing::SoftAuthenticator;
use calimero_node::api::Pending;
use serde_json::{json, Value};

use crate::harness;

use calimero_client::Request;
use harness::{fixture, home_org, kv_wasm, signed_up, typed, wire, Cluster, ORIGIN};
use reqwest::Method;

const PACKAGE: &str = "com.test.notes";

/// What a release declares beyond its code.
#[derive(Default)]
struct Declares<'a> {
    outbound: &'a [&'a str],
    frontend: Option<&'a str>,
    aliases: &'a [&'a str],
    /// `upgrades: publisher`.
    publisher_upgrades: bool,
    /// `uses`, as the manifest's JSON.
    uses: Option<Value>,
    /// The display name (default `Notes`).
    name: Option<&'a str>,
}

/// A signed single-service `.mpk` of `PACKAGE`, and its manifest's hash.
fn release(
    version: &str,
    wasm: &[u8],
    key: &ed25519_dalek::SigningKey,
    d: &Declares<'_>,
) -> (Vec<u8>, String) {
    release_of(PACKAGE, version, wasm, key, d)
}

/// [`release`] of `package`.
fn release_of(
    package: &str,
    version: &str,
    wasm: &[u8],
    key: &ed25519_dalek::SigningKey,
    d: &Declares<'_>,
) -> (Vec<u8>, String) {
    let sha = |b: &[u8]| hex::encode(calimero_identity::sha256(b));
    let manifest = calimero_bundle::BundleManifest {
        version: "1.0".to_owned(),
        package: package.to_owned(),
        app_version: version.to_owned(),
        signer_id: Some(calimero_bundle::derive_signer_id_did_key(
            key.verifying_key().as_bytes(),
        )),
        min_runtime_version: "0.1.0".to_owned(),
        metadata: Some(calimero_bundle::BundleMetadata {
            name: d.name.unwrap_or("Notes").to_owned(),
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
        compute: None,
        links: d.frontend.map(|f| calimero_bundle::BundleLinks {
            frontend: Some(f.to_owned()),
            github: None,
            docs: None,
        }),
        frontend: None,
        frontend_aliases: d.aliases.iter().map(|a| (*a).to_owned()).collect(),
        outbound: d.outbound.iter().map(|u| (*u).to_owned()).collect(),
        secrets: None,
        uses: d.uses.clone().map(|u| serde_json::from_value(u).unwrap()),
        upgrades: d
            .publisher_upgrades
            .then_some(calimero_bundle::UpgradeAuthority::Publisher),
        instance_org: None,
        guests: None,
        services: None,
        pipelines: Vec::new(),
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
pub(crate) fn origin(url: &str) -> String {
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
pub(crate) struct Page {
    http: reqwest::Client,
    url: String,
}

impl Page {
    pub(crate) fn new(url: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            url: url.to_owned(),
        }
    }

    pub(crate) async fn model(&self) -> Value {
        let r = self
            .http
            .get(format!("{}/model", self.url))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        r.json().await.unwrap()
    }

    pub(crate) async fn post(&self, what: &str, body: Value) -> (u16, Value) {
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
    pub(crate) async fn approve(
        &self,
        c: &Cluster,
        key: &mut SoftAuthenticator,
    ) -> ((u16, Value), Value) {
        self.approve_with(c, key, json!({})).await
    }

    /// [`Self::approve`] with `body`'s fields (a chosen `org`) on both posts.
    async fn approve_with(
        &self,
        c: &Cluster,
        key: &mut SoftAuthenticator,
        body: Value,
    ) -> ((u16, Value), Value) {
        let (code, asked) = self.post("approve", body.clone()).await;
        assert_eq!(code, 412, "{asked}");
        assert_eq!(asked["error"], "CONFIRM_REQUIRED");
        let d = &asked["details"];
        let id = d["challengeId"].as_str().unwrap();
        let args = challenge_args(c, id);
        let challenge = calimero_identity::from_b64url(d["challenge"].as_str().unwrap()).unwrap();
        let assertion = key.assert(&challenge);
        let mut confirm = body;
        confirm["confirm"] = json!({ "challengeId": id, "assertion": assertion });
        let answer = self.post("approve", confirm).await;
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
    let c = Cluster::start_with(3, |cfg| {
        cfg.apps_domain = Some("localhost".into());
        cfg.attestation_verifier_url = Some("https://verify.calimero.network".into());
    })
    .await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
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
            ..Declares::default()
        },
    );
    let _ = alice.packages().upload(v1, None).await.unwrap();
    let _ = alice.packages().upload(v2, None).await.unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .map(wire)
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
    let mine = home_org(&alice).await;
    for (path, body) in [
        (format!("apps/{app}/upgrade"), json!({ "version": "2.0.0" })),
        (
            format!("orgs/{mine}/releases"),
            json!({ "version": "9.9.9" }),
        ),
        (
            format!("orgs/{mine}/outbound"),
            json!({ "package": PACKAGE, "prefixes": ["https://metrics.example/"] }),
        ),
    ] {
        let e = in_app
            .transport()
            .json(Request::new(Method::POST, &path).json(body))
            .await
            .unwrap_err();
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
            .consent()
            .page_create(typed(ask(&elsewhere)))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("RETURN_TO_NOT_ALLOWED"), "{elsewhere}: {e}");
    }
    let mut unknown = ask(&format!("{app_origin}/back"));
    unknown["extra"] = json!(1);
    let e = in_app
        .transport()
        .json(Request::new(Method::POST, "consent/requests").json(unknown))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 400, "unknown fields are refused: {e}");
    let made = in_app
        .transport()
        .json(
            Request::new(Method::POST, "consent/requests")
                .json(ask(&format!("{app_origin}/1.0.0/?tab=2"))),
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
    // A POST that names no origin passes only as the browser's own
    // same-origin request (`Sec-Fetch-Site`); anything else, a form another
    // site submits or a client that sends neither header, is refused. Past
    // the gate, `approve` with no assertion asks for one (412), and decides
    // nothing.
    for (origin, fetch_site, want) in [
        (None, None, 403),
        (None, Some("cross-site"), 403),
        (None, Some("same-site"), 403),
        (None, Some("same-origin"), 412),
        (Some(origin(&url)), None, 412),
    ] {
        let mut req = page.http.post(format!("{url}/approve")).json(&json!({}));
        if let Some(o) = &origin {
            req = req.header("origin", o.as_str());
        }
        if let Some(s) = fetch_site {
            req = req.header("sec-fetch-site", s);
        }
        let r = req.send().await.unwrap();
        assert_eq!(
            r.status(),
            want,
            "origin {origin:?}, sec-fetch-site {fetch_site:?}"
        );
    }

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
            "/publisher/signer",
            "/code",
        ],
    );
    // The update's class, policy and approval, apart from the sheet's snapshot.
    let mut snapshot = snapshot;
    let update = snapshot
        .as_object_mut()
        .and_then(|o| o.remove("update"))
        .unwrap();
    assert_eq!(update["approval"]["step"], "only", "{update}");
    let offered_by = snapshot.as_object_mut().and_then(|o| o.remove("offeredBy"));
    assert_eq!(
        offered_by,
        Some(Value::Null),
        "from no deployment, offered by no one"
    );
    assert_eq!(update["policy"]["source"], "default", "{update}");
    assert!(update["class"].is_string(), "{update}");
    // The confirmation code: the same in the create answer and the model,
    // and the documented derivation from the request id.
    assert_eq!(model["code"], made["code"], "{made}");
    assert_eq!(
        model["code"],
        json!(calimero_node::consent_page::confirmation_code(&id))
    );
    let home = alice.get(&format!("apps/{app}")).await.unwrap()["org"].clone();
    assert!(home.is_string(), "{home}");
    let expected = json!({
        "kind": "upgrade",
        "status": "pending",
        "decidedAt": null,
        "rpId": "localhost",
        "requestId": "<redacted>",
        "expiresAt": "<redacted>",
        "approver": { "account": "<redacted>" },
        "code": "<redacted>",
        // The independent verifier, for this node and its release (mdma's
        // link); a test cluster's mock attestation has nothing to verify,
        // which the page says instead of linking.
        "verifier": {
            "url": format!(
                "https://verify.calimero.network/mero-tee?node_url={}&release_tag=mero-tee-vtest",
                url::form_urlencoded::byte_serialize(c.urls[1].trim_end_matches('/').as_bytes())
                    .collect::<String>()
            ),
            "host": "verify.calimero.network",
            "release": "test",
            "mock": true,
        },
        "requestedBy": {
            "package": PACKAGE, "name": "Notes", "publisher": "Example", "returnOrigin": app_origin,
        },
        // Made without naming an organization: in alice's personal one.
        "scope": { "kind": "app", "app": "<redacted>", "name": "notes", "org": home },
        "headline": "Upgrade Notes from 1.0.0 to 2.0.0",
        "title": "Update notes to version 2.0.0",
        // From no deployment: none named.
        "deployment": null,
        "points": [
            "notes moves from version 1.0.0 to 2.0.0, for everyone who uses it.",
            "Until you approve, nothing changes: it keeps running 1.0.0.",
            "This update can send data to a new place: metrics.example.",
        ],
        "attention": [
            {
                "flag": "sendsData",
                "text": "Can send data to new addresses (each still needs your separate approval):",
                "items": ["metrics.example"],
            },
            {
                "flag": "newSignIn",
                "text": "Adds a new sign-in address. Pages there could act in this app as you:",
                "items": ["https://share.example"],
            },
        ],
        // Where the app's unit may send data, classified (`calimero_bundle::update`): a new
        // destination is red; only destinations are reported.
        "unit": {
            "class": "red",
            "reasons": [{ "code": "egressAdded", "destinations": ["https://metrics.example/"] }],
            "egress": {
                "before": ["https://api.example/v1/"],
                "after": ["https://api.example/v1/", "https://metrics.example/"],
                "added": ["https://metrics.example/"],
                "removed": [],
            },
            "stateVersionChanged": false,
            "newMethods": [],
            "removedMethods": [],
            "signerChanged": false,
        },
        "app": { "name": "Notes", "package": PACKAGE },
        "publisher": { "name": "Example", "signer": "<redacted>", "keyChanged": false, "org": null },
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
            "secrets": { "added": [], "removed": [], "kept": [], "bindingChanged": [], "words": [] },
            "uses": { "added": [], "removed": [], "kept": [], "changed": [], "createdAdded": [] },
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
        model["upgrade"]["migration"],
        json!({ "fromStateVersion": 1, "toStateVersion": 1 }),
        "the same state version, read from each component's ABI"
    );
    assert_eq!(model["upgrade"]["abi"], json!({ "from": true, "to": true }));
    assert_eq!(
        model["upgrade"]["methods"],
        json!({ "added": [], "removed": [], "changed": [] }),
        "the same code under another name"
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
    let rec = alice.apps().get(&app).await.map(wire).unwrap();
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
    let result = in_app.consent().page_result(&id).await.map(wire).unwrap();
    assert_eq!(result["status"], "approved");
    let on_other_node = c.client(0);
    on_other_node
        .use_session(in_app.session().await.unwrap())
        .await;
    let result = on_other_node
        .consent()
        .page_result(&id)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(result["status"], "approved", "asked through another node");
    let (bob, _) = signed_up(&c, 2, "bob").await;
    let e = bob.consent().page_result(&id).await.unwrap_err();
    assert_eq!(status(&e), 404);
}

/// An upgrade between two components: the page reads what changes from the
/// `calimero:abi` section each one embeds (methods added, removed and changed,
/// events, timers, the state version), not from an SDK 0.12 schema neither has.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upgrades_page_names_what_the_components_abis_change() {
    let c = Cluster::start(3).await;
    let (alice, _) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    let d = Declares::default();
    let (v1, _) = release("1.0.0", &fixture("apps/fixture-notes-v1"), &publisher, &d);
    let (v2, _) = release("2.0.0", &fixture("apps/fixture-notes-v2"), &publisher, &d);
    let _ = alice.packages().upload(v1, None).await.unwrap();
    let _ = alice.packages().upload(v2, None).await.unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let made = alice
        .consent()
        .page_create(typed(json!({
            "kind": "upgrade",
            "subject": { "app": app, "toVersion": "2.0.0" },
            "returnTo": format!("{ORIGIN}/done"),
        })))
        .await
        .map(wire)
        .unwrap();
    let model = Page::new(made["url"].as_str().unwrap()).model().await;
    let u = &model["upgrade"];
    assert_eq!(u["abi"], json!({ "from": true, "to": true }), "{u}");

    let names = |v: &Value| -> Vec<String> {
        v.as_array()
            .unwrap_or_else(|| panic!("not a list: {v}"))
            .iter()
            .map(|m| m["name"].as_str().or(m.as_str()).unwrap().to_owned())
            .collect()
    };
    let methods = &u["methods"];
    assert_eq!(
        names(&methods["added"]),
        ["exists", "tidy", "wipe"],
        "{methods}"
    );
    assert_eq!(names(&methods["removed"]), ["remove"], "{methods}");
    assert_eq!(names(&methods["changed"]), ["set"], "{methods}");
    let set = &methods["changed"][0];
    assert_eq!(
        (&set["from"]["confirm"], &set["to"]["confirm"]),
        (&json!(false), &json!(true)),
        "{set}"
    );
    assert_eq!(
        set["to"]["params"],
        json!([["id", "string"], ["title", "string"], ["text", "string"]]),
        "{set}"
    );
    let exists = &methods["added"][0];
    assert_eq!(
        (&exists["kind"], &exists["public"]),
        (&json!("view"), &json!(true)),
        "{exists}"
    );
    assert_eq!(
        methods["added"][2],
        json!({
            "name": "wipe", "kind": "call", "confirm": true, "public": false, "rate": null,
            "allowSystem": false, "allowApps": [], "allowAction": false,
            "params": [["id", "string"]], "returns": null,
        })
    );

    assert_eq!(
        u["events"],
        json!({ "added": ["Wiped"], "removed": [], "kept": ["Changed"] })
    );
    assert_eq!(
        u["timers"],
        json!({ "added": [{ "name": "tidy", "everySecs": 3600 }], "removed": [], "changed": [] })
    );
    assert_eq!(
        u["migration"],
        json!({ "fromStateVersion": 1, "toStateVersion": 2 }),
        "an upgrade is code only: no migration runs"
    );

    // What the person is told first.
    let flags: Vec<&str> = model["attention"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["flag"].as_str().unwrap())
        .collect();
    assert_eq!(
        flags,
        ["newPublic", "stateVersion", "removesFeatures"],
        "{model}"
    );
    assert_eq!(model["attention"][0]["items"], json!(["exists"]));
    assert_eq!(model["attention"][2]["items"], json!(["remove"]));
}

/// A bundle re-installed under the version the person was shown is not the
/// one they approve: the page refuses it as stale, and the route refuses an
/// upgrade whose target manifest is not the one named.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_bundle_swapped_under_the_same_version_is_refused() {
    let c = Cluster::start(3).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
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
    let _ = alice.packages().upload(v1, None).await.unwrap();
    let _ = alice.packages().upload(shown, None).await.unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let made = alice
        .consent()
        .page_create(typed(json!({
            "kind": "upgrade",
            "subject": { "app": app, "toVersion": "2.0.0" },
            "returnTo": format!("{ORIGIN}/done"),
        })))
        .await
        .map(wire)
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    assert_eq!(
        page.model().await["upgrade"]["to"]["manifestHash"],
        json!(shown_hash)
    );

    // The same WASM under the same version, another manifest.
    let _ = alice
        .packages()
        .version_uninstall(PACKAGE, "2.0.0")
        .await
        .map(wire)
        .unwrap();
    let _ = alice.packages().upload(swapped, None).await.unwrap();

    let (code, refused) = page.post("approve", json!({})).await;
    assert_eq!(
        (code, refused["error"].as_str()),
        (409, Some("CONSENT_STALE")),
        "{refused}"
    );
    // The route binds whatever manifest the target has now: the swapped one.
    let e = alice
        .apps()
        .upgrade(&app, typed(json!({ "version": "2.0.0" })), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let id = e.details().unwrap()["challengeId"].as_str().unwrap();
    assert_eq!(challenge_args(&c, id)["manifest"], json!(swapped_hash));
    let mut key = SoftAuthenticator::with_key(&alice_key, "localhost", ORIGIN);
    let _ok = alice
        .apps()
        .upgrade(&app, typed(json!({ "version": "2.0.0" })), Some(&mut key))
        .await
        .unwrap();
}

/// The prefix v1 of [`reached_v1_private_v2`] declares.
const OPEN_PREFIX: &str = "https://open.example/api/";

/// Alice's private v1 and v2 of `PACKAGE`, v1 declaring [`OPEN_PREFIX`] and v2
/// `secret_prefix`, and an instance of v1 that anyone reaches; Alice and Bob each in an
/// organization of their own.
async fn reached_v1_private_v2(
    c: &Cluster,
    secret_prefix: &str,
) -> (
    calimero_client::Client,
    calimero_client::Client,
    SoftAuthenticator,
) {
    let (alice, _a) = signed_up(c, 0, "alice").await;
    let (bob, bob_pk) = signed_up(c, 1, "bob").await;
    let _o = alice.orgs().create("Acme", None, None).await.unwrap();
    let _o = bob.orgs().create("Bobco", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[12; 32]);
    for (version, outbound) in [("1.0.0", OPEN_PREFIX), ("2.0.0", secret_prefix)] {
        let (mpk, _) = release(
            version,
            &kv_wasm(),
            &publisher,
            &Declares {
                outbound: &[outbound],
                ..Declares::default()
            },
        );
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let _ = alice
        .apps()
        .create(typed(
            json!({ "name": "open", "package": PACKAGE, "version": "1.0.0",
            "grants": [{ "kind": "anyone" }] }),
        ))
        .await
        .unwrap();
    (alice, bob, bob_pk)
}

/// Reaching an instance of v1 shows the outbound sheet of v1, the newest release Bob reads,
/// never the private v2: v2's prefix is answered as one no release declares.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_outbound_sheet_never_shows_a_release_its_asker_cannot_read() {
    const SECRET: &str = "https://internal.acme.example/secret-api/";
    let c = Cluster::start(3).await;
    let (_alice, bob, _b) = reached_v1_private_v2(&c, SECRET).await;
    let sheet = |subject: Value| {
        bob.consent().page_create(typed(json!({
            "kind": "outbound",
            "scope": "account",
            "subject": subject,
            "returnTo": format!("{ORIGIN}/settings"),
        })))
    };
    let made = sheet(json!({ "package": PACKAGE }))
        .await
        .map(wire)
        .unwrap();
    let model = Page::new(made["url"].as_str().unwrap()).model().await;
    assert_eq!(model["outbound"]["release"], "1.0.0", "{model}");
    assert_eq!(
        model["outbound"]["declared"],
        json!([OPEN_PREFIX]),
        "{model}"
    );
    assert!(!model.to_string().contains(SECRET), "{model}");
    for prefix in [SECRET, "https://guess.example/"] {
        let e = sheet(json!({ "package": PACKAGE, "prefixes": [prefix] }))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("NOT_DECLARED"), "{prefix}: {e}");
    }
}

/// Approving a package's outbound hosts approves the prefixes of the newest release its
/// approver reads: Bob, who reaches only an instance of v1, approves v1's and never learns
/// the private v2's.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_outbound_approval_never_reads_a_release_its_approver_cannot_read() {
    const SECRET: &str = "https://internal.acme.example/secret-api/";
    let c = Cluster::start(3).await;
    let (_alice, bob, mut bob_pk) = reached_v1_private_v2(&c, SECRET).await;
    let bobs = home_org(&bob).await;
    let got = bob
        .orgs()
        .outbound_approval(
            &bobs,
            PACKAGE,
            Some(typed(json!("manifest"))),
            None,
            Some(&mut bob_pk),
        )
        .await
        .map(wire)
        .unwrap();
    assert_eq!(got["approval"]["prefixes"], json!([OPEN_PREFIX]), "{got}");
    assert!(!got.to_string().contains(SECRET), "{got}");
}

/// Reaching an instance of one package reads nothing of another package shipping the same
/// WASM at the same version: Bob runs the operator's `notes@1.0.0`, and approving outbound
/// hosts of Acme's private `secret@1.0.0` answers as for a package not installed.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_outbound_approval_never_reads_another_packages_release_of_the_same_wasm() {
    const SECRET: &str = "https://internal.acme.example/secret-api/";
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let _o = alice.orgs().create("Acme", None, None).await.unwrap();
    let _o = bob.orgs().create("Bobco", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[16; 32]);
    let secret = Declares {
        outbound: &[SECRET],
        ..Declares::default()
    };
    let (mpk, _) = release_of("com.test.secret", "1.0.0", &kv_wasm(), &publisher, &secret);
    let _ = alice.packages().upload(mpk, None).await.unwrap();
    let (mpk, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = ops.ops().package_upload(mpk).await.unwrap();
    let _ = bob
        .apps()
        .create(typed(
            json!({ "name": "n", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .unwrap();

    let bobs = home_org(&bob).await;
    let mut answers = Vec::new();
    for package in ["com.test.secret", "com.test.missing"] {
        let got = bob
            .orgs()
            .outbound_approval(
                &bobs,
                package,
                Some(typed(json!("manifest"))),
                None,
                Some(&mut bob_pk),
            )
            .await
            .map(wire)
            .map_err(|e| e.to_string());
        answers.push(got);
    }
    assert!(!format!("{answers:?}").contains(SECRET), "{answers:?}");
    assert_eq!(answers[0], answers[1]);
}

/// A session bound to a package learns nothing of a release its account may not read: Bob,
/// signed in on the origin of Alice's private package, asks for an organization, and the
/// page names neither the release's display name nor its author and offers no app team, as
/// for a package not installed; a sign-up begun on that origin names the package id alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_strangers_bound_session_never_shows_a_private_releases_name() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (_bob, bob_key) = signed_up(&c, 1, "bob").await;
    let _o = alice.orgs().create("Acme", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[13; 32]);
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let _ = alice.packages().upload(v1, None).await.unwrap();

    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(1);
    let mut bob_in_app = SoftAuthenticator::with_key(&bob_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut bob_in_app).await.unwrap();
    let made = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "createOrg", "subject": { "name": "bobs" },
            "returnTo": format!("{app_origin}/1.0.0/") }),
    )
    .await
    .unwrap();
    let model = Page::new(made["url"].as_str().unwrap()).model().await;
    assert_eq!(model["requestedBy"]["name"], Value::Null, "{model}");
    assert_eq!(model["requestedBy"]["publisher"], Value::Null, "{model}");
    assert_eq!(model["appTeam"], Value::Null, "{model}");
    assert!(!model.to_string().contains("Notes"), "{model}");

    let begun = post(
        &c.client(2),
        "auth/passkey/register/begin",
        json!({ "name": "carol", "origin": app_origin }),
    )
    .await
    .unwrap();
    let shown = begun["options"]["user"]["name"].as_str().unwrap();
    assert!(!shown.contains("Notes"), "{shown}");
}

/// A sign-up on a package's origin names its newest release open to anyone: the operator's
/// 1.0.0, never the private 2.0.0 Acme installed since.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sign_up_names_the_newest_release_open_to_anyone() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let _o = alice.orgs().create("Acme", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[14; 32]);
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = ops.ops().package_upload(v1).await.unwrap();
    let private = Declares {
        name: Some("Acme Internal"),
        ..Declares::default()
    };
    let (v2, _) = release("2.0.0", &variant(&kv_wasm(), "v2"), &publisher, &private);
    let _ = alice.packages().upload(v2, None).await.unwrap();

    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let begun = post(
        &c.client(2),
        "auth/passkey/register/begin",
        json!({ "name": "carol", "origin": app_origin }),
    )
    .await
    .unwrap();
    let shown = begun["options"]["user"]["name"].as_str().unwrap();
    assert!(shown.contains("Notes"), "{shown}");
    assert!(!shown.contains("Acme Internal"), "{shown}");
}

/// The app list names the release an instance runs, never a private sibling that shares its
/// WASM: Bob's instance of the operator's 1.0.0 lists as 1.0.0, though Acme's private 2.0.0
/// shipped that WASM first.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_app_list_never_names_a_private_sibling_sharing_the_wasm() {
    let c = Cluster::start(3).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let _o = alice.orgs().create("Acme", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[15; 32]);
    let (v2, _) = release("2.0.0", &kv_wasm(), &publisher, &Declares::default());
    let _ = alice.packages().upload(v2, None).await.unwrap();
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = ops.ops().package_upload(v1).await.unwrap();
    let app = bob
        .apps()
        .create(typed(
            json!({ "name": "n", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .unwrap()
        .app_id;

    let listed = bob.apps().list(None, None).await.map(wire).unwrap();
    let row = listed["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["appId"] == json!(app))
        .unwrap();
    assert_eq!(row["version"], json!("1.0.0"), "{listed}");
    assert!(!listed.to_string().contains("2.0.0"), "{listed}");
}

/// The app list filters by the package an instance runs, never by the package its WASM was
/// first recorded under: Bob's instance of the operator's `notes@1.0.0` shares its WASM with
/// Acme's private `secret@1.0.0`, installed first, and lists under notes only, to his account
/// and to a session bound to the notes frontend, in `GET /apps` and the account's report.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_app_list_filters_by_the_package_an_instance_runs() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, bob_key) = signed_up(&c, 1, "bob").await;
    let _o = alice.orgs().create("Acme", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[17; 32]);
    let (mpk, _) = release_of(
        "com.test.secret",
        "1.0.0",
        &kv_wasm(),
        &publisher,
        &Declares::default(),
    );
    let _ = alice.packages().upload(mpk, None).await.unwrap();
    let (mpk, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ = ops.ops().package_upload(mpk).await.unwrap();
    let app = bob
        .apps()
        .create(typed(
            json!({ "name": "n", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .unwrap()
        .app_id;
    let ids = |listed: Value| -> Vec<Value> {
        listed["apps"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["appId"].clone())
            .collect()
    };
    let list = |package| bob.apps().list(package, None);
    let secret = ids(list(Some("com.test.secret")).await.map(wire).unwrap());
    let missing = ids(list(Some("com.test.missing")).await.map(wire).unwrap());
    let notes = ids(list(Some(PACKAGE)).await.map(wire).unwrap());
    assert_eq!(secret, missing);
    assert_eq!(notes, vec![json!(app)]);

    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(1);
    let mut bob_in_app = SoftAuthenticator::with_key(&bob_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut bob_in_app).await.unwrap();
    let bound = ids(in_app.apps().list(None, None).await.map(wire).unwrap());
    assert_eq!(bound, vec![json!(app)]);
    let report = in_app
        .auth()
        .account_apps(None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(report["apps"][0]["app"], json!(app), "{report}");
    assert_eq!(report["apps"].as_array().unwrap().len(), 1, "{report}");
}

/// Release and outbound requests through the page, a person's own account
/// as the scope; the bounds a request is held to.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn releases_and_outbound_prefixes_are_approved_on_the_page_within_bounds() {
    let c = Cluster::start(3).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    // The account's own scope is its only organization.
    let _home = home_org(&alice).await;
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
    let _ = alice.packages().upload(v1, None).await.unwrap();
    let back = format!("{ORIGIN}/settings");

    // A release: the model says what changes before and after.
    let made = alice
        .consent()
        .page_create(typed(json!({
            "kind": "release",
            "scope": "account",
            "subject": { "version": "2.0.0" },
            "returnTo": back,
        })))
        .await
        .map(wire)
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["kind"], "release");
    assert_eq!(
        model["verifier"],
        Value::Null,
        "no verifier configured, no link"
    );
    assert!(
        model["before"].as_str().unwrap().starts_with(
            "Until you approve, nodes running mero-tee 2.0.0 receive none of the data"
        ),
        "{model}"
    );
    assert_eq!(model["release"]["alreadyApproved"], false);
    assert_eq!(
        model["title"],
        "Allow a new secure server version (2.0.0) to hold your data"
    );
    assert_eq!(model["points"][0], "Cluster servers running version 2.0.0 will be allowed to receive, store and run your apps.");
    assert_eq!(model["attention"], json!([]));
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
    let mine = home_org(&alice).await;
    let me = get_until(&alice, &format!("orgs/{mine}"), |o| {
        o["approvedReleases"]
            .as_array()
            .is_some_and(|a| a.contains(&json!("2.0.0")))
    })
    .await;
    assert!(
        me["approvedReleases"]
            .as_array()
            .unwrap()
            .contains(&json!("2.0.0")),
        "{me}"
    );

    // Outbound: exactly the prefixes listed, never "whatever is declared".
    let made = alice
        .consent()
        .page_create(typed(json!({
            "kind": "outbound",
            "scope": "account",
            "subject": { "package": PACKAGE, "prefixes": ["https://api.example/"] },
            "returnTo": back,
        })))
        .await
        .map(wire)
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["outbound"]["new"], json!(["https://api.example/"]));
    assert_eq!(model["title"], "Allow Notes to send data to api.example");
    assert_eq!(model["app"], json!({ "name": "Notes", "package": PACKAGE }));
    assert_eq!(model["publisher"]["name"], "Example");
    let ((code, done), args) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["prefixes"], json!(["https://api.example/"]));
    let e = alice
        .consent()
        .page_create(typed(json!({
            "kind": "outbound",
            "scope": "account",
            "subject": { "package": PACKAGE, "prefixes": ["https://undeclared.example/"] },
            "returnTo": back,
        })))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_DECLARED"), "{e}");

    // Declined: the app is sent back with the result, and nothing is approved.
    let made = alice
        .consent()
        .page_create(typed(json!({
            "kind": "release",
            "scope": "account",
            "subject": { "version": "3.0.0" },
            "returnTo": back,
        })))
        .await
        .map(wire)
        .unwrap();
    let id = made["requestId"].as_str().unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let (code, declined) = page.post("decline", json!({})).await;
    assert_eq!(code, 200);
    assert_eq!(
        declined["redirect"],
        json!(format!("{back}?request_id={id}&result=declined"))
    );
    let result = alice.consent().page_result(id).await.map(wire).unwrap();
    assert_eq!(result["status"], "declined");

    // One undecided request per account and node: a new one replaces it, so
    // a page swapped for an older request has nothing to approve, and the app
    // that asked first finds its request gone.
    let ask = json!({
        "kind": "release",
        "scope": "account",
        "subject": { "version": "4.0.0" },
        "returnTo": back,
    });
    let older = alice
        .consent()
        .page_create(typed(ask.clone()))
        .await
        .map(wire)
        .unwrap();
    let older_id = older["requestId"].as_str().unwrap().to_owned();
    let newer = alice
        .consent()
        .page_create(typed(ask))
        .await
        .map(wire)
        .unwrap();
    let e = alice.consent().page_result(&older_id).await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    let r = reqwest::get(format!("{}/model", older["url"].as_str().unwrap()))
        .await
        .unwrap();
    assert_eq!(r.status(), 404, "the replaced request has no page");
    let newer_page = Page::new(newer["url"].as_str().unwrap());
    assert_eq!(newer_page.model().await["status"], "pending");

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
    // A request with no organization (`account`) is her personal one's.
    let mine = home_org(&alice).await;
    let approved = || async {
        alice
            .orgs()
            .get(&mine, None, None, None)
            .await
            .map(wire)
            .unwrap()["approvedReleases"]
            .as_array()
            .unwrap()
            .clone()
    };
    let (alice_ref, mine_ref) = (&alice, &mine);
    let approved_until = |want: Vec<Value>| async move {
        let o = get_until(alice_ref, &format!("orgs/{mine_ref}"), |o| {
            o["approvedReleases"].as_array() == Some(&want)
        })
        .await;
        o["approvedReleases"].as_array().unwrap().clone()
    };
    let first = approved().await;
    assert_eq!(first.len(), 1, "the sign-up node's release: {first:?}");
    let first = first[0].as_str().unwrap().to_owned();
    let mut page_key = SoftAuthenticator::with_key(&alice_key, "localhost", &origin(&c.urls[0]));

    // A second release, approved on the page.
    let made = alice
        .consent()
        .page_create(typed(ask("2.0.0", false)))
        .await
        .map(wire)
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let ((code, done), _) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    let o = get_until(&alice, &format!("orgs/{mine}"), |o| {
        o["approvedReleases"]
            .as_array()
            .is_some_and(|a| a.contains(&json!("2.0.0")))
    })
    .await;
    assert!(o["approvedReleases"]
        .as_array()
        .unwrap()
        .contains(&json!("2.0.0")));

    // Withdrawn on the page: no measurements, the version alone is bound.
    let made = alice
        .consent()
        .page_create(typed(ask("2.0.0", true)))
        .await
        .map(wire)
        .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["kind"], "release");
    assert_eq!(model["release"]["change"], "withdraw", "{model}");
    assert_eq!(
        model["title"],
        "Stop server version 2.0.0 from receiving your data"
    );
    assert_eq!(model["attention"], json!([]), "not the last approved");
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
    assert_eq!(approved_until(vec![json!(first)]).await, vec![json!(first)]);

    // A release that is not approved: the page says withdrawing changes nothing.
    let made = alice
        .consent()
        .page_create(typed(ask("5.0.0", true)))
        .await
        .map(wire)
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
        .consent()
        .page_create(typed(ask(&first, true)))
        .await
        .map(wire)
        .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["release"]["lastApproved"], true, "{model}");
    assert_eq!(model["attention"][0]["flag"], "lastApproved", "{model}");
    let ((code, refused), _) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 400, "{refused}");
    assert_eq!(refused["error"], "REJECTED");
    assert_eq!(
        refused["message"],
        "the last approved release cannot be withdrawn"
    );
    assert_eq!(approved_until(vec![json!(first)]).await, vec![json!(first)]);
    let result = alice.consent().page_result(&id).await.map(wire).unwrap();
    assert_eq!(result["status"], "pending");

    // The subject is still closed.
    let mut unknown = ask("2.0.0", true);
    unknown["subject"]["approved"] = json!(false);
    let e = alice
        .transport()
        .json(Request::new(Method::POST, "consent/requests").json(unknown))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 400, "{e}");
}

/// An organization's release on the page: approving is `APPROVE_RELEASES`,
/// judged when the request is made (a member without it is refused before
/// any page exists) and again on the page (a capability revoked meanwhile is
/// refused before the passkey); the owner approves; the operator's deadline
/// for the release is in the model and its plain words.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_organizations_release_is_approved_on_the_page_by_whoever_approves_releases() {
    const APPROVE_RELEASES: u32 = 1 << 10;
    let c = Cluster::start(3).await;
    let (alice, mut alice_key) = signed_up(&c, 0, "alice").await;
    let (bob, _) = signed_up(&c, 0, "bob").await;
    let org = alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .orgs()
        .invite_create(
            &org,
            typed(json!({ "role": "member" })),
            Some(&mut alice_key),
        )
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ok = harness::ops_signed(
        &ops,
        "ops/releases/3.0.0/deadline",
        json!({ "after": "2999-01-01T00:00:00Z" }),
    )
    .await
    .unwrap();
    let ask = json!({
        "kind": "release",
        "scope": { "org": org },
        "subject": { "version": "3.0.0" },
        "returnTo": format!("{ORIGIN}/settings"),
    });

    // A member without the capability: no page at all.
    let e = bob
        .consent()
        .page_create(typed(ask.clone()))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert!(e.to_string().contains("APPROVE_RELEASES"), "{e}");

    // Granted, then revoked while the page is open: refused before the passkey.
    let _ok = alice
        .orgs()
        .capabilities(
            &org,
            u64::from(APPROVE_RELEASES),
            Some(&bob_id),
            None,
            Some(&mut alice_key),
        )
        .await
        .unwrap();
    let made = bob
        .consent()
        .page_create(typed(ask.clone()))
        .await
        .map(wire)
        .unwrap();
    let _ok = alice
        .orgs()
        .capabilities(&org, 0, Some(&bob_id), None, Some(&mut alice_key))
        .await
        .unwrap();
    let (code, refused) = Page::new(made["url"].as_str().unwrap())
        .post("approve", json!({}))
        .await;
    assert_eq!(code, 403, "{refused}");
    assert!(
        refused["message"]
            .as_str()
            .unwrap()
            .contains("APPROVE_RELEASES"),
        "{refused}"
    );

    // The owner: the deadline in the model and in plain words; approved.
    let made = alice
        .consent()
        .page_create(typed(json!({
            "kind": "release",
            "scope": { "org": org },
            "subject": { "version": "3.0.0" },
            "returnTo": format!("{ORIGIN}/settings"),
        })))
        .await
        .map(wire)
        .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(
        model["title"],
        "Allow a new secure server version (3.0.0) to hold Acme's data"
    );
    assert_eq!(
        model["release"]["deadline"],
        json!({ "after": "2999-01-01T00:00:00Z", "afterSecs": 32_472_144_000_u64, "passed": false }),
        "{model}"
    );
    let points: Vec<&str> = model["points"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        points.iter().any(|p| p.starts_with(
            "After Jan 1, 2999 (UTC), new apps, installs and updates for Acme's apps pause"
        )),
        "{points:?}"
    );
    let mut page_key = SoftAuthenticator::with_key(&alice_key, "localhost", &origin(&c.urls[0]));
    let ((code, done), args) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["version"], "3.0.0");
    let view = get_until(&alice, &format!("orgs/{org}"), |o| {
        o["approvedReleases"]
            .as_array()
            .is_some_and(|a| a.contains(&json!("3.0.0")))
    })
    .await;
    assert!(
        view["approvedReleases"]
            .as_array()
            .unwrap()
            .contains(&json!("3.0.0")),
        "{view}"
    );
}

/// A package its publisher upgrades is its organization's owner's and admins' to
/// upgrade on the page as on the route (`Feature::OwnerApprovedUpgrades`), its key
/// bound or not; an owner-upgraded app is never moved back to a publisher release,
/// refused before any page exists (403).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_publisher_upgraded_package_is_upgraded_on_its_organizations_page() {
    let c = Cluster::start(3).await;
    let (alice, _) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[12; 32]);
    let (v1, _) = release(
        "1.0.0",
        &kv_wasm(),
        &publisher,
        &Declares {
            publisher_upgrades: true,
            ..Declares::default()
        },
    );
    let (v2, _) = release(
        "2.0.0",
        &variant(&kv_wasm(), "v2"),
        &publisher,
        &Declares::default(),
    );
    let _ = alice.packages().upload(v1, None).await.unwrap();
    let _ = alice.packages().upload(v2, None).await.unwrap();
    let create =
        |name: &str, version: &str| json!({ "name": name, "package": PACKAGE, "version": version });
    let app_of = |v: Value| v["appId"].as_str().unwrap().to_owned();
    let held = app_of(
        alice
            .transport()
            .json(Request::new(Method::POST, "apps").json(create("held", "1.0.0")))
            .await
            .unwrap(),
    );
    let owned = app_of(
        alice
            .transport()
            .json(Request::new(Method::POST, "apps").json(create("owned", "2.0.0")))
            .await
            .unwrap(),
    );
    let ask = |app: &str, to: &str| {
        json!({
            "kind": "upgrade",
            "subject": { "app": app, "toVersion": to },
            "returnTo": format!("{ORIGIN}/settings"),
        })
    };

    // The key is bound to no organization: the instance's own organization decides.
    let _page = alice
        .consent()
        .page_create(typed(ask(&held, "2.0.0")))
        .await
        .unwrap();

    let e = alice
        .consent()
        .page_create(typed(ask(&owned, "1.0.0")))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert!(e.to_string().contains("its publisher upgrades"), "{e}");
}

/// `POST path` with `body` over the client's sealed transport, as JSON.
#[allow(
    clippy::result_large_err,
    reason = "the client's own error, passed through for the test to read its status"
)]
async fn post(
    c: &calimero_client::Client,
    path: &str,
    body: Value,
) -> Result<Value, calimero_client::Error> {
    c.transport()
        .json(Request::new(Method::POST, path).json(body))
        .await
}

/// `GET path` until `done` holds of the answer: an approval is written
/// through one node, and the node asked applies it a moment later.
async fn get_until(
    c: &calimero_client::Client,
    path: &str,
    done: impl Fn(&Value) -> bool,
) -> Value {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let last = get(c, path).await;
        if let Ok(v) = &last {
            if done(v) {
                return v.clone();
            }
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{path} never got there; last answer {last:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

/// `GET path` over the client's sealed transport, as JSON.
#[allow(
    clippy::result_large_err,
    reason = "the client's own error, passed through for the test to read its status"
)]
async fn get(c: &calimero_client::Client, path: &str) -> Result<Value, calimero_client::Error> {
    c.transport().json(Request::new(Method::GET, path)).await
}

/// An app's frontend manages organizations only through the node's page: its
/// package-bound session is refused `POST /orgs`, invites and redemption
/// (the chat app's "Create workspace"), and asks the node instead. The
/// person approves each with their passkey on the node's page; what was made
/// (the organization's id, the invite's token, the organization joined)
/// reaches the asking app through the sealed `/result` only.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_apps_frontend_creates_an_organization_invites_and_joins_on_the_page() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let (bob, bob_key) = signed_up(&c, 2, "bob").await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let _ = alice.packages().upload(v1, None).await.unwrap();

    // Alice and Bob in the app's own frontend: sessions bound to the package.
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let back = format!("{app_origin}/1.0.0/");
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let bob_in_app = c.client(1);
    let mut bob_in_app_passkey = SoftAuthenticator::with_key(&bob_key, "localhost", &app_origin);
    let _s = bob_in_app.sign_in(&mut bob_in_app_passkey).await.unwrap();

    // What the chat app hit: the frontend may not create an organization.
    let e = post(&in_app, "orgs", json!({ "name": "calimero" }))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");

    // It asks the node; a name the log would refuse never gets a page.
    let ask =
        |kind: &str, subject: Value| json!({ "kind": kind, "subject": subject, "returnTo": back });
    let e = post(
        &in_app,
        "consent/requests",
        ask("createOrg", json!({ "name": " " })),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 400, "{e}");
    let made = post(
        &in_app,
        "consent/requests",
        ask("createOrg", json!({ "name": "calimero" })),
    )
    .await
    .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["kind"], "createOrg", "{model}");
    assert_eq!(model["title"], "Create the organization calimero");
    assert_eq!(model["requestedBy"]["package"], PACKAGE);
    // Asked from the app's frontend, the organization comes with the app's
    // own team, and the page says so.
    assert_eq!(model["appTeam"]["package"], PACKAGE, "{model}");
    let result = get(&in_app, &format!("consent/{id}/result")).await.unwrap();
    assert_eq!(result["status"], "pending");
    assert!(result.get("outcome").is_none(), "{result}");

    // The same passkey (one signature counter) on the node's origin.
    let mut alice_page = SoftAuthenticator::with_key(&alice_in_app, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(
        args,
        json!({ "name": "calimero", "appTeam": PACKAGE, "__method": "__create_org" })
    );
    assert_eq!(
        done["redirect"],
        json!(format!("{back}?request_id={id}&result=approved"))
    );
    let result = get(&in_app, &format!("consent/{id}/result")).await.unwrap();
    assert_eq!(result["status"], "approved", "{result}");
    assert_eq!(result["kind"], "createOrg");
    let org = result["outcome"]["orgId"].as_str().unwrap().to_owned();
    let alice_id = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let o = get_until(&alice, &format!("orgs/{org}"), |o| o["name"] == "calimero").await;
    assert_eq!(o["name"], "calimero");
    assert_eq!(o["members"][&alice_id], "owner", "{o}");
    // Decided once.
    let (code, again) = page.post("approve", json!({})).await;
    assert_eq!(code, 409, "{again}");
    // One approval made the app's team too, open, with Alice its lead; the
    // frontend runs its channels below it without another page.
    let team = result["outcome"]["teamId"].as_str().unwrap().to_owned();
    let t = &o["teams"][&team];
    assert_eq!(t["metadata"]["app"]["package"], PACKAGE, "{o}");
    assert_eq!(t["join"], "everyone", "{t}");
    assert_eq!(t["members"][&alice_id], true, "{t}");
    let channel = post(
        &in_app,
        &format!("orgs/{org}/teams"),
        json!({ "name": "general", "join": "invite", "parent": team }),
    )
    .await
    .unwrap();
    assert!(channel["teamId"].is_string(), "{channel}");
    let e = post(
        &in_app,
        &format!("orgs/{org}/teams"),
        json!({ "name": "ops", "join": "invite" }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403, "not an org-wide team: {e}");

    // An invite: refused directly, made on the page, its token to the app only.
    let e = post(
        &in_app,
        &format!("orgs/{org}/invites"),
        json!({ "role": "member" }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let e = post(
        &in_app,
        "consent/requests",
        ask("createInvite", json!({ "org": org, "role": "owner" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403, "never an owner invite: {e}");
    let e = post(
        &bob_in_app,
        "consent/requests",
        ask("createInvite", json!({ "org": org, "role": "member" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 404, "not a member of it: {e}");
    let made = post(
        &in_app,
        "consent/requests",
        ask(
            "createInvite",
            json!({ "org": org, "role": "member", "uses": 2 }),
        ),
    )
    .await
    .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["kind"], "createInvite", "{model}");
    assert_eq!(model["title"], "Invite people to calimero");
    assert_eq!(model["invite"]["uses"], 2);
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__create_invite", "{args}");
    assert!(
        !done.to_string().contains("inv_"),
        "the page's clear answer carries no token: {done}"
    );
    let result = get(&in_app, &format!("consent/{id}/result")).await.unwrap();
    let token = result["outcome"]["token"].as_str().unwrap().to_owned();
    assert!(token.starts_with("inv_"), "{result}");
    // Another account's app does not read it.
    let e = get(&bob_in_app, &format!("consent/{id}/result"))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "{e}");

    // Bob joins with it: refused directly, approved on the page.
    let e = post(&bob_in_app, &format!("invites/{token}/redeem"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let e = post(
        &bob_in_app,
        "consent/requests",
        ask("acceptInvite", json!({ "token": "inv_unknown" })),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    let made = post(
        &bob_in_app,
        "consent/requests",
        ask("acceptInvite", json!({ "token": token })),
    )
    .await
    .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["kind"], "acceptInvite", "{model}");
    assert_eq!(model["title"], "Join calimero");
    assert!(!model.to_string().contains(&token), "{model}");
    // Alice's passkey is not Bob's: the page refuses it.
    let mut wrong = SoftAuthenticator::with_key(&alice_page, "localhost", &origin(&url));
    let ((code, refused), _) = page.approve(&c, &mut wrong).await;
    assert_ne!(code, 200, "{refused}");
    let mut bob_page = SoftAuthenticator::with_key(&bob_in_app_passkey, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut bob_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__accept_invite", "{args}");
    let result = get(&bob_in_app, &format!("consent/{id}/result"))
        .await
        .unwrap();
    assert_eq!(result["outcome"]["org"], json!(org), "{result}");
    let o = get_until(&alice, &format!("orgs/{org}"), |o| {
        o["members"][&bob_id] == "member"
    })
    .await;
    assert_eq!(o["members"][&bob_id], "member", "{o}");
}

/// The organization routes these tests call, over the client's sealed
/// transport (the hand-written methods are gone).
#[allow(
    clippy::result_large_err,
    reason = "the client's own error, passed through for the test to read its status"
)]
trait Routes {
    async fn post(&self, path: &str, body: Value) -> Result<Value, calimero_client::Error>;
    async fn get(&self, path: &str) -> Result<Value, calimero_client::Error>;
    async fn delete(&self, path: &str) -> Result<Value, calimero_client::Error>;
    async fn delete_with_body(
        &self,
        path: &str,
        body: Value,
    ) -> Result<Value, calimero_client::Error>;
    async fn post_confirmed(
        &self,
        path: &str,
        body: Value,
        passkey: &mut SoftAuthenticator,
    ) -> Result<Value, calimero_client::Error>;
}

#[allow(
    clippy::result_large_err,
    reason = "the client's own error, passed through for the test to read its status"
)]
impl Routes for calimero_client::Client {
    async fn post(&self, path: &str, body: Value) -> Result<Value, calimero_client::Error> {
        post(self, path, body).await
    }

    async fn get(&self, path: &str) -> Result<Value, calimero_client::Error> {
        get(self, path).await
    }

    async fn delete(&self, path: &str) -> Result<Value, calimero_client::Error> {
        self.transport()
            .json(Request::new(Method::DELETE, path))
            .await
    }

    async fn delete_with_body(
        &self,
        path: &str,
        body: Value,
    ) -> Result<Value, calimero_client::Error> {
        self.transport()
            .json(Request::new(Method::DELETE, path).json(body))
            .await
    }

    async fn post_confirmed(
        &self,
        path: &str,
        body: Value,
        passkey: &mut SoftAuthenticator,
    ) -> Result<Value, calimero_client::Error> {
        self.transport()
            .confirmed(Request::new(Method::POST, path).json(body), passkey)
            .await
    }
}

/// A consent request of `kind` over `subject` from `app`, returning to
/// `back`: its answer, or the refusal's status.
async fn ask(
    app: &calimero_client::Client,
    back: &str,
    kind: &str,
    subject: Value,
) -> Result<Value, u16> {
    app.post(
        "consent/requests",
        json!({ "kind": kind, "subject": subject, "returnTo": back }),
    )
    .await
    .map_err(|e| status(&e))
}

/// Ask `kind` from `app` and approve it on the node's page with `key` (the
/// person's passkey there): the page's model, the arguments its challenge
/// committed to, and what `/result` answers the app.
async fn approved(
    c: &Cluster,
    app: &calimero_client::Client,
    back: &str,
    kind: &str,
    subject: Value,
    key: &mut SoftAuthenticator,
) -> (Value, Value, Value) {
    let made = ask(app, back, kind, subject).await.unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let page = Page::new(made["url"].as_str().unwrap());
    let model = page.model().await;
    assert_eq!(model["kind"], kind, "{model}");
    let ((code, done), args) = page.approve(c, key).await;
    assert_eq!(code, 200, "{kind}: {done}");
    let result = app.get(&format!("consent/{id}/result")).await.unwrap();
    assert_eq!(result["status"], "approved", "{result}");
    assert_eq!(result["kind"], kind, "{result}");
    (model, args, result)
}

/// A refused request's status.
async fn refused(app: &calimero_client::Client, back: &str, kind: &str, subject: Value) -> u16 {
    ask(app, back, kind, subject).await.unwrap_err()
}

/// The account id `c` is signed in as.
async fn account_of(c: &calimero_client::Client) -> String {
    c.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A chat app (its frontend: package-bound sessions) and its "server" (an
/// organization) that Alice owns, with Bob and Carol as members and a team.
/// The setup goes through Alice's own client, before the app's sessions copy
/// the passkeys.
struct Server {
    c: Cluster,
    org: String,
    team: String,
    alice: calimero_client::Client,
    alice_id: String,
    bob_id: String,
    carol_id: String,
    /// The app's sessions.
    alice_app: calimero_client::Client,
    bob_app: calimero_client::Client,
    carol_app: calimero_client::Client,
    /// Each person's passkey on the node's consent page.
    alice_page: SoftAuthenticator,
    bob_page: SoftAuthenticator,
    back: String,
}

impl Server {
    async fn start() -> Self {
        let c = Cluster::start_with(3, |cfg| {
            cfg.apps_domain = Some("localhost".into());
            cfg.attestation_verifier_url = Some("https://verify.calimero.network".into());
        })
        .await;
        let (alice, mut alice_key) = signed_up(&c, 0, "alice").await;
        let _org = alice.orgs().create("alice org", None, None).await.unwrap();
        let (bob, bob_key) = signed_up(&c, 2, "bob").await;
        let (carol, carol_key) = signed_up(&c, 2, "carol").await;
        let (alice_id, bob_id, carol_id) = (
            account_of(&alice).await,
            account_of(&bob).await,
            account_of(&carol).await,
        );
        let publisher = ed25519_dalek::SigningKey::from_bytes(&[12; 32]);
        let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
        let _ = alice.packages().upload(v1, None).await.unwrap();

        let org = alice
            .post("orgs", json!({ "name": "guild" }))
            .await
            .unwrap()["orgId"]
            .as_str()
            .unwrap()
            .to_owned();
        let team = alice
            .post(
                &format!("orgs/{org}/teams"),
                json!({ "name": "general", "join": "invite" }),
            )
            .await
            .unwrap()["teamId"]
            .as_str()
            .unwrap()
            .to_owned();
        for who in [&bob, &carol] {
            let inv = alice
                .post_confirmed(
                    &format!("orgs/{org}/invites"),
                    json!({ "role": "member" }),
                    &mut alice_key,
                )
                .await
                .unwrap();
            let token = inv["token"].as_str().unwrap();
            let _r = who
                .post(&format!("invites/{token}/redeem"), json!({}))
                .await
                .unwrap();
        }
        // Bob's name in the server, as its admins see him.
        let _r = bob
            .post(
                &format!("orgs/{org}/members/{bob_id}/metadata"),
                json!({ "metadata": { "name": "Bobby" } }),
            )
            .await
            .unwrap();

        let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
        let app_origin = format!(
            "http://{}.localhost:{port}",
            calimero_node::app_host::label(PACKAGE)
        );
        let back = format!("{app_origin}/1.0.0/");
        let page_origin = origin(&c.urls[1]);
        let signed_in_app = |key: &SoftAuthenticator| {
            let client = c.client(1);
            let in_app = SoftAuthenticator::with_key(key, "localhost", &app_origin);
            (client, in_app)
        };
        let (alice_app, mut a) = signed_in_app(&alice_key);
        let _s = alice_app.sign_in(&mut a).await.unwrap();
        let (bob_app, mut b) = signed_in_app(&bob_key);
        let _s = bob_app.sign_in(&mut b).await.unwrap();
        let (carol_app, mut k) = signed_in_app(&carol_key);
        let _s = carol_app.sign_in(&mut k).await.unwrap();
        Self {
            alice_page: SoftAuthenticator::with_key(&a, "localhost", &page_origin),
            bob_page: SoftAuthenticator::with_key(&b, "localhost", &page_origin),
            c,
            org,
            team,
            alice,
            alice_id,
            bob_id,
            carol_id,
            alice_app,
            bob_app,
            carol_app,
            back,
        }
    }

    /// What `/orgs/{org}` answers Alice once `done` holds of it.
    async fn org_until(&self, done: impl Fn(&Value) -> bool) -> Value {
        get_until(&self.alice, &format!("orgs/{}", self.org), done).await
    }

    /// What `/orgs/{org}/invites` answers Alice once `done` holds of it.
    async fn invites_until(&self, done: impl Fn(&Value) -> bool) -> Value {
        get_until(&self.alice, &format!("orgs/{}/invites", self.org), done).await
    }
}

/// A chat app's moderation, from its own frontend (package-bound sessions,
/// refused every organization route): roles, teams, invites, removing a
/// member and leaving, each asked of the node and approved on its page with
/// the person's passkey, after the same checks the log applies.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_apps_frontend_moderates_its_server_on_the_page() {
    let mut s = Server::start().await;
    let (org, team, back) = (s.org.clone(), s.team.clone(), s.back.clone());
    let (bob, carol) = (s.bob_id.clone(), s.carol_id.clone());
    let alice_app = &s.alice_app;

    // setMemberRole: Bob becomes an admin.
    let e = alice_app
        .post(
            &format!("orgs/{org}/members"),
            json!({ "account": bob, "role": "admin" }),
        )
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let role = |role: &str| json!({ "org": org, "account": bob, "role": role });
    assert_eq!(
        refused(&s.carol_app, &back, "setMemberRole", role("admin")).await,
        403,
        "a member manages nobody"
    );
    assert_eq!(
        refused(&s.bob_app, &back, "setMemberRole", role("owner")).await,
        403,
        "only the owner hands ownership over"
    );
    assert_eq!(
        refused(alice_app, &back, "setMemberRole", role("member")).await,
        409,
        "nothing would change"
    );
    let (model, args, result) = approved(
        &s.c,
        alice_app,
        &back,
        "setMemberRole",
        role("admin"),
        &mut s.alice_page,
    )
    .await;
    assert_eq!(model["title"], "Make Bobby an admin of guild", "{model}");
    // The organization kinds carry the verifier link like every kind.
    assert_eq!(
        model["verifier"]["host"], "verify.calimero.network",
        "{model}"
    );
    assert_eq!(model["verifier"]["mock"], true, "{model}");
    assert_eq!(
        args,
        json!({ "account": bob, "role": "admin", "__method": "__set_org_role" })
    );
    assert!(result.get("outcome").is_none(), "{result}");
    let _o = s.org_until(|o| o["members"][&bob] == "admin").await;

    // setMemberTeams: Carol in #general.
    let teams = json!({ "org": org, "account": carol, "teams": [{ "team": team }] });
    let e = alice_app
        .post(
            &format!("orgs/{org}/members/{carol}/teams"),
            json!({ "teams": [{ "team": team }] }),
        )
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert_eq!(
        refused(&s.carol_app, &back, "setMemberTeams", teams.clone()).await,
        403,
        "a member manages no team"
    );
    assert_eq!(
        refused(
            alice_app,
            &back,
            "setMemberTeams",
            json!({ "org": org, "account": carol, "teams": [] })
        )
        .await,
        400
    );
    let (model, args, _) = approved(
        &s.c,
        alice_app,
        &back,
        "setMemberTeams",
        teams,
        &mut s.alice_page,
    )
    .await;
    assert_eq!(model["title"], "Put carol in general", "{model}");
    assert_eq!(args["__method"], "__set_team_members", "{args}");
    assert_eq!(args["teams"], json!([{ "team": team }]));
    let o = s
        .org_until(|o| o["teams"][&team]["members"].get(&carol).is_some())
        .await;
    assert_eq!(
        o["teams"][&team]["members"][&carol], false,
        "a row, not a lead"
    );

    // removeMemberTeams: Carol out of #general again.
    let out = json!({ "org": org, "account": carol, "teams": [team] });
    let e = alice_app
        .delete_with_body(
            &format!("orgs/{org}/members/{carol}/teams"),
            json!({ "teams": [team] }),
        )
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert_eq!(
        refused(
            &s.carol_app,
            &back,
            "removeMemberTeams",
            json!({ "org": org, "account": bob, "teams": [team] })
        )
        .await,
        403,
        "a member takes nobody else out of a team"
    );
    let (model, args, _) = approved(
        &s.c,
        alice_app,
        &back,
        "removeMemberTeams",
        out,
        &mut s.alice_page,
    )
    .await;
    assert_eq!(model["title"], "Take carol out of general", "{model}");
    assert_eq!(args["__method"], "__remove_team_members", "{args}");
    let _o = s
        .org_until(|o| o["teams"][&team]["members"].get(&carol).is_none())
        .await;

    // revokeInvite: an invite made earlier stops working.
    let invites = s.invites_until(|i| i["invites"] == json!([])).await;
    assert_eq!(invites["invites"], json!([]), "both were used: {invites}");
    let made = ask(
        alice_app,
        &back,
        "createInvite",
        json!({ "org": org, "role": "guest" }),
    )
    .await
    .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let ((code, done), _) = page.approve(&s.c, &mut s.alice_page).await;
    assert_eq!(code, 200, "{done}");
    let invites = s.invites_until(|i| i["invites"][0].is_object()).await;
    let key = invites["invites"][0]["inviteKey"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = alice_app
        .delete(&format!("orgs/{org}/invites/{key}"))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert_eq!(
        refused(
            alice_app,
            &back,
            "revokeInvite",
            json!({ "org": org, "inviteKey": "0".repeat(64) })
        )
        .await,
        404
    );
    assert_eq!(
        refused(
            &s.carol_app,
            &back,
            "revokeInvite",
            json!({ "org": org, "inviteKey": key })
        )
        .await,
        404,
        "a member does not see an admin's invite"
    );
    let (model, args, _) = approved(
        &s.c,
        alice_app,
        &back,
        "revokeInvite",
        json!({ "org": org, "inviteKey": key }),
        &mut s.alice_page,
    )
    .await;
    assert_eq!(model["title"], "Revoke an invite to guild", "{model}");
    assert_eq!(model["invite"]["createdBy"], "you", "{model}");
    assert_eq!(
        args,
        json!({ "inviteKey": key, "__method": "__revoke_invite" })
    );
    let invites = s.invites_until(|i| i["invites"] == json!([])).await;
    assert_eq!(invites["invites"], json!([]), "{invites}");

    // removeMember: Carol is kicked.
    let e = alice_app
        .delete(&format!("orgs/{org}/members/{carol}"))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let kick = |who: &str| json!({ "org": org, "account": who });
    assert_eq!(
        refused(&s.carol_app, &back, "removeMember", kick(&bob)).await,
        403,
        "a member removes nobody"
    );
    assert_eq!(
        refused(&s.bob_app, &back, "removeMember", kick(&s.alice_id)).await,
        403,
        "never the owner"
    );
    assert_eq!(
        refused(alice_app, &back, "removeMember", kick("nobody")).await,
        404
    );
    let (model, args, _) = approved(
        &s.c,
        alice_app,
        &back,
        "removeMember",
        kick(&carol),
        &mut s.alice_page,
    )
    .await;
    assert_eq!(model["title"], "Remove carol from guild", "{model}");
    assert_eq!(
        args,
        json!({ "account": carol, "__method": "__remove_member" })
    );
    let _o = s.org_until(|o| o["members"].get(&carol).is_none()).await;

    // leaveOrg: Bob leaves; the owner cannot.
    let e = s
        .bob_app
        .delete(&format!("orgs/{org}/members/{bob}"))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert_eq!(
        refused(alice_app, &back, "leaveOrg", json!({ "org": org })).await,
        409,
        "the owner hands ownership over first"
    );
    assert_eq!(
        refused(&s.carol_app, &back, "leaveOrg", json!({ "org": org })).await,
        404,
        "not in it any more"
    );
    let bob_app = &s.bob_app;
    let (model, args, _) = approved(
        &s.c,
        bob_app,
        &back,
        "leaveOrg",
        json!({ "org": org }),
        &mut s.bob_page,
    )
    .await;
    assert_eq!(model["title"], "Leave guild", "{model}");
    assert_eq!(model["member"]["role"], "admin", "{model}");
    assert_eq!(
        args,
        json!({ "account": bob, "__method": "__remove_member" })
    );
    let _o = s.org_until(|o| o["members"].get(&bob).is_none()).await;
}

/// A chat app's server settings, from its own frontend: rename, what members
/// and teams may do, every app added later, and deleting it (the page counts
/// what goes with it).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_apps_frontend_changes_and_deletes_its_server_on_the_page() {
    let mut s = Server::start().await;
    let (org, team, back) = (s.org.clone(), s.team.clone(), s.back.clone());
    let bob = s.bob_id.clone();
    let alice_app = &s.alice_app;

    // renameOrg.
    let e = alice_app
        .post(&format!("orgs/{org}/name"), json!({ "name": "hall" }))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let name = |n: &str| json!({ "org": org, "name": n });
    assert_eq!(
        refused(&s.bob_app, &back, "renameOrg", name("hall")).await,
        403,
        "a member renames nothing"
    );
    assert_eq!(refused(alice_app, &back, "renameOrg", name(" ")).await, 400);
    let (model, args, result) = approved(
        &s.c,
        alice_app,
        &back,
        "renameOrg",
        name("hall"),
        &mut s.alice_page,
    )
    .await;
    assert_eq!(model["title"], "Rename guild to hall", "{model}");
    assert_eq!(args, json!({ "name": "hall", "__method": "__rename_org" }));
    assert_eq!(
        result["outcome"],
        json!({ "orgId": org, "name": "hall" }),
        "{result}"
    );
    let _o = s.org_until(|o| o["name"] == "hall").await;

    // setCapabilities, a member's: Bob may create apps and invite people.
    let e = alice_app
        .post(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": bob, "capabilities": 3 }),
        )
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let caps = json!({ "org": org, "account": bob, "capabilities": 3 });
    assert_eq!(
        refused(&s.bob_app, &back, "setCapabilities", caps.clone()).await,
        403
    );
    let (model, args, _) = approved(
        &s.c,
        alice_app,
        &back,
        "setCapabilities",
        caps,
        &mut s.alice_page,
    )
    .await;
    assert_eq!(
        model["title"], "Change what Bobby across hall can do",
        "{model}"
    );
    assert_eq!(
        args,
        json!({ "team": null, "account": bob, "capabilities": 3, "__method": "__set_capabilities" })
    );
    let _o = s.org_until(|o| o["capabilities"][&bob] == 3).await;
    // ... and a team's default.
    let default = json!({ "org": org, "team": team, "capabilities": 1 });
    assert_eq!(
        refused(&s.carol_app, &back, "setCapabilities", default.clone()).await,
        404,
        "a team she does not see"
    );
    let (model, args, _) = approved(
        &s.c,
        alice_app,
        &back,
        "setCapabilities",
        default,
        &mut s.alice_page,
    )
    .await;
    assert_eq!(
        model["title"], "Change what everyone in general can do by default",
        "{model}"
    );
    assert_eq!(args["__method"], "__set_default_capabilities", "{args}");
    let _o = s
        .org_until(|o| o["teams"][&team]["defaultCapabilities"] == 1)
        .await;

    // setAllApps.
    let e = alice_app
        .post(
            &format!("orgs/{org}/all-apps"),
            json!({ "account": bob, "allApps": true }),
        )
        .await
        .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let all = |on: bool| json!({ "org": org, "account": bob, "allApps": on });
    assert_eq!(
        refused(&s.bob_app, &back, "setAllApps", all(true)).await,
        403
    );
    assert_eq!(
        refused(alice_app, &back, "setAllApps", all(false)).await,
        409,
        "nothing would change"
    );
    let (model, args, _) = approved(
        &s.c,
        alice_app,
        &back,
        "setAllApps",
        all(true),
        &mut s.alice_page,
    )
    .await;
    assert_eq!(
        model["title"], "Give Bobby every app added to hall",
        "{model}"
    );
    assert_eq!(args["__method"], "__set_all_apps", "{args}");
    let _o = s.org_until(|o| o["allApps"] == json!([bob])).await;

    // deleteOrg: a stale page is refused, then the page counts what goes.
    let e = alice_app.delete(&format!("orgs/{org}")).await.unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    let gone = json!({ "org": org });
    assert_eq!(
        refused(&s.bob_app, &back, "deleteOrg", gone.clone()).await,
        403,
        "only the owner deletes it"
    );
    let made = ask(alice_app, &back, "deleteOrg", gone.clone())
        .await
        .unwrap();
    let stale = Page::new(made["url"].as_str().unwrap());
    // An invite made after the page counted them, by Bob (who may invite
    // now): a request of Alice's own would replace hers, as one account
    // holds one undecided request per node.
    let made = ask(
        &s.bob_app,
        &back,
        "createInvite",
        json!({ "org": org, "role": "member" }),
    )
    .await
    .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let ((code, done), _) = page.approve(&s.c, &mut s.bob_page).await;
    assert_eq!(code, 200, "{done}");
    let (code, answer) = stale.post("approve", json!({})).await;
    assert_eq!(code, 409, "{answer}");
    assert_eq!(answer["error"], "CONSENT_STALE", "{answer}");
    let (model, args, result) =
        approved(&s.c, alice_app, &back, "deleteOrg", gone, &mut s.alice_page).await;
    assert_eq!(
        model["title"], "Delete hall and everything in it",
        "{model}"
    );
    assert_eq!(
        model["delete"],
        json!({
            "org": org, "members": 3, "teams": 1, "invites": 1, "apps": 0, "appNames": [],
        })
    );
    assert_eq!(model["attention"][0]["flag"], "deletes", "{model}");
    assert_eq!(
        model["attention"][0]["text"],
        "Deleting hall deletes its 1 team, 1 invite and 0 apps, for good."
    );
    assert_eq!(args, json!({ "org": org, "__method": "__delete_org" }));
    assert_eq!(result["outcome"], json!({ "apps": [] }), "{result}");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
    let e = loop {
        match s.alice.get(&format!("orgs/{org}")).await {
            Err(e) => break e,
            Ok(_) => assert!(
                tokio::time::Instant::now() < deadline,
                "the org is still there"
            ),
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    assert_eq!(status(&e), 404, "{e}");
}

/// A package's capability card is readable before any app of it exists: by
/// an account that sees the release (the signed manifest's facts and the
/// service component's ABI, verbatim), and by a session bound to the package
/// even for an account that sees no release of it; any other account, and an
/// unknown version, are told there is no such version.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_package_versions_capabilities_are_readable_before_an_app_exists() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, _alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let (bob, bob_key) = signed_up(&c, 0, "bob").await;
    let (_carol, carol_key) = signed_up(&c, 0, "carol").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let wasm = kv_wasm();
    let (mpk, manifest_hash) = release(
        "1.0.0",
        &wasm,
        &publisher,
        &Declares {
            outbound: &["https://metrics.example/", "https://api.example/v1/"],
            ..Declares::default()
        },
    );
    let mpk_hash = hex::encode(calimero_identity::sha256(&mpk));
    let _ = alice.packages().upload(mpk, None).await.unwrap();
    let path = format!("packages/{PACKAGE}/1.0.0/capabilities");

    let card = get(&alice, &path).await.unwrap();
    let abi = calimero_node::component::embedded_abi(&wasm).expect("the fixture embeds an ABI");
    assert!(abi["methods"].as_object().is_some_and(|m| !m.is_empty()));
    assert_eq!(
        card,
        json!({
            "package": PACKAGE,
            "version": "1.0.0",
            "service": null,
            "bundle": hex::encode(calimero_identity::sha256(&wasm)),
            "mpk": mpk_hash,
            "manifestHash": manifest_hash,
            "signer": calimero_bundle::derive_signer_id_did_key(publisher.verifying_key().as_bytes()),
            "name": "Notes",
            "author": "Example",
            "outbound": ["https://api.example/v1/", "https://metrics.example/"],
            "secrets": {},
            "secretBindings": [],
            "uses": {},
            "abi": abi,
            "destinations": ["https://api.example/v1/", "https://metrics.example/"],
            "members": {},
        })
    );

    // Bob shares no organization with the installer: no such version.
    let e = get(&bob, &path).await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    assert!(e.to_string().contains("no such package version"), "{e}");
    let e = get(&alice, &format!("packages/{PACKAGE}/9.9.9/capabilities"))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    assert!(e.to_string().contains("no such package version"), "{e}");

    // Bob in the package's own frontend: a session bound to it sees what Bob sees.
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(1);
    let mut in_app_passkey = SoftAuthenticator::with_key(&bob_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut in_app_passkey).await.unwrap();
    assert_eq!(status(&get(&in_app, &path).await.unwrap_err()), 404);

    // Carol, bound to another package's frontend, sees nothing of this one.
    let (other, _) = release_of(
        "com.test.other",
        "1.0.0",
        &wasm,
        &publisher,
        &Declares::default(),
    );
    let _ = alice.packages().upload(other, None).await.unwrap();
    let other_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label("com.test.other")
    );
    let elsewhere = c.client(1);
    let mut elsewhere_passkey = SoftAuthenticator::with_key(&carol_key, "localhost", &other_origin);
    let _s = elsewhere.sign_in(&mut elsewhere_passkey).await.unwrap();
    let e = get(&elsewhere, &path).await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    assert!(e.to_string().contains("no such package version"), "{e}");
}

/// A member who saw a release through the installer's organization stops
/// seeing it once removed from that organization.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_member_removed_from_the_org_loses_the_capability_card() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_key) = signed_up(&c, 0, "alice").await;
    let (amy, _amy_key) = signed_up(&c, 1, "amy").await;
    let org = alice
        .orgs()
        .create("acme", None, None)
        .await
        .unwrap()
        .org_id;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let (mpk, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let _ = alice.packages().upload(mpk, None).await.unwrap();
    let path = format!("packages/{PACKAGE}/1.0.0/capabilities");
    assert_eq!(status(&get(&amy, &path).await.unwrap_err()), 404);
    let invite = alice
        .orgs()
        .invite_create(
            &org,
            typed(json!({ "role": "member" })),
            Some(&mut alice_key),
        )
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = amy.orgs().invite_redeem(&invite).await.unwrap();
    assert_eq!(get(&amy, &path).await.unwrap()["version"], "1.0.0");

    let _removed = alice
        .orgs()
        .remove_member(&org, &account_of(&amy).await, Some(&mut alice_key))
        .await
        .unwrap();
    let e = get(&amy, &path).await.unwrap_err();
    assert_eq!(status(&e), 404, "{e}");
    assert!(e.to_string().contains("no such package version"), "{e}");
}

/// An app's frontend sets its new organization's member defaults in the same
/// approval that creates it (`createOrg { capabilities }`): the page says in
/// words what members can do, the step-up commits to the bits, and the
/// organization exists with them. Absent, nothing changes; the plain
/// `POST /orgs` takes the same field.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_frontend_creates_an_organization_with_member_capabilities_in_one_approval() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let _ = alice.packages().upload(v1, None).await.unwrap();
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let back = format!("{app_origin}/1.0.0/");
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();

    let create = |subject: Value| ask(&in_app, &back, "createOrg", subject);
    let wanted = 0b111; // create apps, invite people, join open teams
    assert_eq!(
        status_of(create(json!({ "name": "x", "capabilities": -1 })).await),
        400,
        "bits are an unsigned number"
    );
    assert_eq!(
        status_of(create(json!({ "name": "x", "capabilities": 1 << 20 })).await),
        400,
        "a bit no capability has"
    );
    let e = post(
        &alice,
        "orgs",
        json!({ "name": "x", "capabilities": 1 << 20 }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 400, "{e}");
    assert_eq!(e.code(), Some("UNKNOWN_CAPABILITIES"), "{e}");
    let made = create(json!({ "name": "calimero", "capabilities": wanted }))
        .await
        .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["capabilities"]["bits"], wanted, "{model}");
    let points = model["points"].to_string();
    assert!(
        points.contains("Members can create apps, invite people and join open teams."),
        "{points}"
    );
    let mut alice_page = SoftAuthenticator::with_key(&alice_in_app, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(
        args,
        json!({
            "name": "calimero", "appTeam": PACKAGE, "capabilities": wanted,
            "__method": "__create_org"
        })
    );
    let id = made["requestId"].as_str().unwrap();
    let result = get(&in_app, &format!("consent/{id}/result")).await.unwrap();
    let org = result["outcome"]["orgId"].as_str().unwrap().to_owned();
    let o = get_until(&alice, &format!("orgs/{org}"), |o| {
        o["defaultCapabilities"] == wanted
    })
    .await;
    assert_eq!(o["defaultCapabilities"], wanted, "{o}");

    // Absent: the page names no capabilities and the default is a new org's.
    let made = create(json!({ "name": "plain" })).await.unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    assert!(page.model().await.get("capabilities").is_none());
    let ((_, done), args) = page.approve(&c, &mut alice_page).await;
    assert!(args.get("capabilities").is_none(), "{args} {done}");
    let id = made["requestId"].as_str().unwrap();
    let result = get(&in_app, &format!("consent/{id}/result")).await.unwrap();
    let org = result["outcome"]["orgId"].as_str().unwrap();
    let o = get_until(&alice, &format!("orgs/{org}"), |o| o["name"] == "plain").await;
    assert_eq!(o["defaultCapabilities"], 4, "{o}");

    // The plain route, unbound session.
    let made = post(
        &alice,
        "orgs",
        json!({ "name": "direct", "capabilities": 3 }),
    )
    .await
    .unwrap();
    let direct = format!("orgs/{}", made["orgId"].as_str().unwrap());
    let o = get_until(&alice, &direct, |o| o["name"] == "direct").await;
    assert_eq!(o["defaultCapabilities"], 3, "{o}");
}

/// A refused request's status, from [`ask`]'s answer.
fn status_of(r: Result<Value, u16>) -> u16 {
    r.expect_err("the request is refused")
}

/// An invite made on the page carries capabilities: the page says in words
/// what whoever joins can do (the joiner's page too), the step-up commits to
/// the bits, and the member gets exactly them. Nobody below admin hands out
/// a capability they lack, on the page or on the plain route.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_invite_carries_capabilities_to_the_member_who_joins_with_it() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let (bob, bob_key) = signed_up(&c, 2, "bob").await;
    let (carol, carol_key) = signed_up(&c, 2, "carol").await;
    let carol_id = account_of(&carol).await;
    let bob_id = bob.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[11; 32]);
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let _ = alice.packages().upload(v1, None).await.unwrap();
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let back = format!("{app_origin}/1.0.0/");
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let bob_in_app = c.client(1);
    let mut bob_in_app_passkey = SoftAuthenticator::with_key(&bob_key, "localhost", &app_origin);
    let _s = bob_in_app.sign_in(&mut bob_in_app_passkey).await.unwrap();

    let org = post(&alice, "orgs", json!({ "name": "acme" }))
        .await
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let granted = 0b11; // create apps, invite people

    let made = ask(
        &in_app,
        &back,
        "createInvite",
        json!({ "org": org, "role": "member", "capabilities": granted }),
    )
    .await
    .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["invite"]["capabilities"], granted, "{model}");
    assert!(
        model["points"]
            .to_string()
            .contains("Whoever joins can create apps and invite people."),
        "{model}"
    );
    let mut alice_page = SoftAuthenticator::with_key(&alice_in_app, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["capabilities"], granted, "{args}");
    let result = get(&in_app, &format!("consent/{id}/result")).await.unwrap();
    let token = result["outcome"]["token"].as_str().unwrap().to_owned();

    let made = ask(
        &bob_in_app,
        &back,
        "acceptInvite",
        json!({ "token": token }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert!(
        model["points"]
            .to_string()
            .contains("You can create apps and invite people."),
        "{model}"
    );
    let mut bob_page = SoftAuthenticator::with_key(&bob_in_app_passkey, "localhost", &origin(&url));
    let ((code, done), _) = page.approve(&c, &mut bob_page).await;
    assert_eq!(code, 200, "{done}");
    let o = get_until(&alice, &format!("orgs/{org}"), |o| {
        o["capabilities"].get(&bob_id).is_some()
    })
    .await;
    assert_eq!(o["capabilities"][&bob_id], granted, "{o}");

    // A bit no capability has is refused, on the page and on the route.
    let unknown = json!({ "role": "member", "capabilities": 1 << 20 });
    let e = post(&alice, &format!("orgs/{org}/invites"), unknown.clone())
        .await
        .unwrap_err();
    assert_eq!(status(&e), 400, "{e}");
    assert_eq!(e.code(), Some("UNKNOWN_CAPABILITIES"), "{e}");
    let e = post(
        &in_app,
        "consent/requests",
        json!({
            "kind": "createInvite",
            "subject": { "org": org, "role": "member", "capabilities": 1 << 20 },
            "returnTo": back,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 400, "{e}");
    assert_eq!(e.code(), Some("UNKNOWN_CAPABILITIES"), "{e}");

    // Bob holds create-apps and invite, nothing else: he grants no more.
    let over = granted | 0b1000; // and manage members
    let invites = format!("orgs/{org}/invites");
    let e = post(
        &bob,
        &invites,
        json!({ "role": "member", "capabilities": over }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert_eq!(e.code(), Some("CANNOT_GRANT_UNHELD"), "{e}");
    let e = post(
        &bob_in_app,
        "consent/requests",
        json!({
            "kind": "createInvite",
            "subject": { "org": org, "role": "member", "capabilities": over },
            "returnTo": back,
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 403, "{e}");
    assert_eq!(e.code(), Some("CANNOT_GRANT_UNHELD"), "{e}");
    let made = ask(
        &bob_in_app,
        &back,
        "createInvite",
        json!({ "org": org, "role": "member", "capabilities": 1 }),
    )
    .await
    .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let url = made["url"].as_str().unwrap().to_owned();
    let ((code, done), _) = Page::new(&url).approve(&c, &mut bob_page).await;
    assert_eq!(code, 200, "{done}");
    let token = get(&bob_in_app, &format!("consent/{id}/result"))
        .await
        .unwrap()["outcome"]["token"]
        .as_str()
        .unwrap()
        .to_owned();

    // Alice strips Bob's capabilities before anyone redeems his invite: it
    // grants no more than he could now, on the page and in the log.
    let _ = approved(
        &c,
        &in_app,
        &back,
        "setCapabilities",
        json!({ "org": org, "account": bob_id, "capabilities": 0 }),
        &mut alice_page,
    )
    .await;
    let _o = get_until(&alice, &format!("orgs/{org}"), |o| {
        o["capabilities"][&bob_id] == 0
    })
    .await;
    let carol_in_app = c.client(1);
    let mut carol_in_app_passkey =
        SoftAuthenticator::with_key(&carol_key, "localhost", &app_origin);
    let _s = carol_in_app
        .sign_in(&mut carol_in_app_passkey)
        .await
        .unwrap();
    let made = ask(
        &carol_in_app,
        &back,
        "acceptInvite",
        json!({ "token": token }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let mut carol_page =
        SoftAuthenticator::with_key(&carol_in_app_passkey, "localhost", &origin(&url));
    let ((code, done), _) = Page::new(&url).approve(&c, &mut carol_page).await;
    assert_eq!(code, 200, "{done}");
    let o = get_until(&alice, &format!("orgs/{org}"), |o| {
        o["members"].get(&carol_id).is_some()
    })
    .await;
    assert_eq!(o["capabilities"][&carol_id], 0, "clamped to Bob's: {o}");
}

/// A sign-in from a package's frontend reaches a node that has committed the
/// package's install but not applied it yet: the node judges the origin on
/// fresh state rather than refusing it as unknown (`401 origin does not
/// match`, the intermittent failure of
/// `a_frontend_creates_an_organization_with_member_capabilities_in_one_approval`).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_sign_in_from_a_frontend_installed_a_moment_ago_reaches_a_node_behind() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let leader = (0..3)
        .find(|i| c.nodes[*i].as_ref().unwrap().node.system.status().is_leader)
        .unwrap();
    let behind = (0..3).find(|i| *i != leader).unwrap();
    let (alice, alice_key) = signed_up(&c, leader, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let behind_node = &c.nodes[behind].as_ref().unwrap().node;
    crate::harness::poll::until(
        crate::harness::poll::EVENTUALLY,
        "the node behind applied the sign-up",
        || behind_node.sys(|s| !s.accounts.is_empty()),
    )
    .await;

    // The node commits the install with the others but applies nothing yet.
    let dir = c._dirs[behind].path().to_owned();
    calimero_log::testing::hold_apply(&dir, calimero_node::node::SYSTEM_GROUP);
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[37; 32]);
    let (v1, _) = release("1.0.0", &kv_wasm(), &publisher, &Declares::default());
    let _ = alice.packages().upload(v1, None).await.unwrap();
    assert!(
        behind_node.sys(|s| !s.packages.contains_key(PACKAGE)),
        "the node must not have applied the install yet"
    );

    let port = url::Url::parse(&c.urls[behind]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(behind);
    let sign_in = tokio::spawn(async move {
        let mut pk = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
        in_app.sign_in(&mut pk).await.map(|_| ())
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    calimero_log::testing::release_apply(&dir, calimero_node::node::SYSTEM_GROUP);
    let signed_in = sign_in.await.unwrap();
    assert!(
        signed_in.is_ok(),
        "the frontend's origin is judged on fresh state: {:?}",
        signed_in.err()
    );
}

/// A unit upgrades on the page in one approval: an app's frontend asks `upgradeUnit`, the
/// page shows the instance's capability sheet and each member's that moves, and the one
/// passkey assertion moves them all (the consent of `__upgrade_unit`, member targets
/// included).
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_unit_upgrades_on_the_page_in_one_approval() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[23; 32]);
    let kv = "com.test.unit-kv";
    let uses = |version: &str| Declares {
        uses: Some(json!({ "kv": { "package": kv, "range": "^0.1", "version": version } })),
        ..Declares::default()
    };
    for (package, version, d) in [
        (kv, "0.1.0", Declares::default()),
        (kv, "0.1.1", Declares::default()),
        (PACKAGE, "1.0.0", uses("0.1.0")),
        (PACKAGE, "2.0.0", uses("0.1.1")),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let made = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap();
    let notes = made["appId"].as_str().unwrap().to_owned();
    let member = made["links"]["kv"].as_str().unwrap().to_owned();

    // The app's frontend asks; the person approves on the node's page.
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let back = format!("{app_origin}/1.0.0/");
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let made = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "upgradeUnit", "subject": { "app": notes, "toVersion": "2.0.0" }, "returnTo": back }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["kind"], "upgradeUnit", "{model}");
    assert_eq!(
        model["title"], "Update notes and the apps it uses",
        "{model}"
    );
    let apps = model["apps"].as_array().unwrap();
    assert_eq!(apps.len(), 2, "{model}");
    assert_eq!(apps[1]["upgrade"]["to"]["version"], "0.1.1", "{model}");
    let mut alice_page = SoftAuthenticator::with_key(&alice_in_app, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__upgrade_unit", "{args}");
    assert_eq!(args["targets"][&member]["version"], "0.1.1", "{args}");

    for (app, version) in [(&notes, "2.0.0"), (&member, "0.1.1")] {
        let _moved = get_until(&alice, &format!("apps/{app}"), |a| {
            a["release"]["version"] == version
        })
        .await;
    }
}

/// A unit's available update tells a reader who may not use the release a member would move
/// to nothing of it: the operator's notes 2.0.0 moves Alice's member to Acme's private kv
/// 0.1.1, which declares a secret host; Bob reaches the instance and sees the update without
/// that host, Acme's owner and members with it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_units_update_never_shows_a_reader_a_member_release_it_may_not_use() {
    const SECRET: &str = "https://internal.acme.example/secret-api/";
    let c = Cluster::start(3).await;
    let (alice, mut alice_key) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let (carol, _c) = signed_up(&c, 2, "carol").await;
    let org = alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let token = alice
        .orgs()
        .invite_create(
            &org,
            typed(json!({ "role": "member" })),
            Some(&mut alice_key),
        )
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = carol.orgs().invite_redeem(&token).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
    let kv = "com.test.unit-kv";
    let uses = |version: &str| Declares {
        uses: Some(json!({ "kv": { "package": kv, "range": "^0.1", "version": version } })),
        ..Declares::default()
    };
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    for (package, version, d) in [
        (kv, "0.1.0", Declares::default()),
        (PACKAGE, "1.0.0", uses("0.1.0")),
        (PACKAGE, "2.0.0", uses("0.1.1")),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = ops.ops().package_upload(mpk).await.unwrap();
    }
    let secret = Declares {
        outbound: &[SECRET],
        ..Declares::default()
    };
    let (mpk, _) = release_of(kv, "0.1.1", &kv_wasm(), &publisher, &secret);
    let _ = alice.packages().upload(mpk, None).await.unwrap();
    let made = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0",
            "grants": [{ "kind": "anyone" }] }),
        ))
        .await
        .map(wire)
        .unwrap();
    let notes = made["appId"].as_str().unwrap().to_owned();

    for org_member in [&alice, &carol] {
        let got = get(org_member, &format!("apps/{notes}/update"))
            .await
            .unwrap();
        assert_eq!(got["available"]["version"], "2.0.0", "{got}");
        assert!(got.to_string().contains(SECRET), "{got}");
    }
    let bobs = get(&bob, &format!("apps/{notes}/update")).await.unwrap();
    assert_eq!(bobs["available"]["version"], "2.0.0", "{bobs}");
    assert!(!bobs.to_string().contains(SECRET), "{bobs}");
}

/// An app's frontend asks the person to let it call another app as them: the page names both
/// apps and every method the app's release lists for the other's package, the person's passkey
/// covers exactly those (`__delegate`), and the grant is recorded. The same frontend may not
/// grant it through the API by itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_apps_frontend_asks_to_act_as_the_person_in_another_app_on_the_page() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[41; 32]);
    let cal = "com.test.delegated-cal";
    let plugin = Declares {
        uses: Some(
            json!({ "cal": { "package": cal, "range": "^1", "mode": "attach",
                                    "methods": ["recordWho", "whoView"] } }),
        ),
        ..Declares::default()
    };
    for (package, version, d) in [
        (cal, "1.0.0", Declares::default()),
        (PACKAGE, "1.0.0", plugin),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let create = |name: &str, package: &str| {
        typed(json!({ "name": name, "package": package, "version": "1.0.0" }))
    };
    let app_of = |v: Value| v["appId"].as_str().unwrap().to_owned();
    let source = app_of(
        alice
            .apps()
            .create(create("booking", PACKAGE))
            .await
            .map(wire)
            .unwrap(),
    );
    let target = app_of(
        alice
            .apps()
            .create(create("calendar", cal))
            .await
            .map(wire)
            .unwrap(),
    );

    // Alice signs in on the plugin's own frontend.
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();

    // The frontend cannot grant itself the power: it asks on the page.
    let refused = in_app
        .apps()
        .delegate(&source, &target, Some(&mut alice_in_app))
        .await
        .unwrap_err();
    assert!(
        matches!(refused, calimero_client::Error::Api { status: 403, .. }),
        "{refused}"
    );
    let made = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "delegate", "subject": { "app": source, "target": target },
                "returnTo": format!("{app_origin}/1.0.0/") }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["kind"], "delegate", "{model}");
    assert_eq!(
        model["title"], "Allow booking to act as you in calendar",
        "{model}"
    );
    assert_eq!(
        model["delegate"]["methods"],
        json!(["recordWho", "whoView"]),
        "{model}"
    );
    assert_eq!(model["delegate"]["target"]["app"], json!(target), "{model}");

    let mut alice_page = SoftAuthenticator::with_key(&alice_in_app, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__delegate", "{args}");
    assert_eq!(args["target"], json!(target), "{args}");
    assert_eq!(args["methods"], json!(["recordWho", "whoView"]), "{args}");

    let listed = get_until(&alice, &format!("apps/{source}/delegations"), |v| {
        v["delegations"][&target].is_array()
    })
    .await;
    assert_eq!(
        listed["delegations"][&target],
        json!(["recordWho", "whoView"]),
        "{listed}"
    );
}

/// An upgrade whose new release takes an attached app in another range warns on the page: an
/// attached app lives on its own, so the approval does not move it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upgrade_warns_when_an_attached_app_falls_outside_its_range() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[29; 32]);
    let cal = "com.test.attached-cal";
    let uses = |range: &str| Declares {
        uses: Some(json!({ "calendar": { "package": cal, "range": range, "mode": "attach" } })),
        ..Declares::default()
    };
    for (package, version, d) in [
        (cal, "2.0.0", Declares::default()),
        (PACKAGE, "1.0.0", uses("^2")),
        (PACKAGE, "2.0.0", uses("^3")),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let create = |name: &str, package: &str, version: &str| {
        typed(json!({ "name": name, "package": package, "version": version }))
    };
    let notes = alice
        .apps()
        .create(create("notes", PACKAGE, "1.0.0"))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let calendar = alice
        .apps()
        .create(create("calendar", cal, "2.0.0"))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let body: calimero_client::types::AppLinkRequest =
        serde_json::from_value(json!({ "app": calendar })).unwrap();
    let got = alice
        .apps()
        .link(&notes, "calendar", body, Some(&mut alice_key))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(got["inRange"], json!(true), "{got}");

    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let made = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "upgrade", "subject": { "app": notes, "toVersion": "2.0.0" },
                "returnTo": format!("{app_origin}/1.0.0/") }),
    )
    .await
    .unwrap();
    let model = Page::new(made["url"].as_str().unwrap()).model().await;
    assert_eq!(
        model["upgrade"]["attachedOutOfRange"],
        json!([{ "name": "calendar", "app": calendar, "package": cal,
                 "version": "2.0.0", "range": "^3" }]),
        "{model}"
    );
    let flagged = model["attention"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["flag"] == "attachedOutOfRange")
        .cloned()
        .unwrap_or_else(|| panic!("no warning: {model}"));
    assert_eq!(
        flagged["items"],
        json!([format!("calendar runs {cal} 2.0.0, outside ^3")]),
        "{flagged}"
    );
}

/// An instance from a deployment, upgraded from inside its app: its frontend asks for the
/// release the developer now offers (naming no version, or naming that one), and the
/// customer's owner approves it on the node's page, though they never installed or saw
/// that release. Another version is refused `409 OUTSIDE_DEPLOYMENT`, and a member of the
/// customer's organization, who created the instance, is refused `403` before any page.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_customer_accepts_its_deployments_offer_on_the_page() {
    const CREATE_APP: u64 = 1 << 0;
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (dev, mut dev_pk) = signed_up(&c, 0, "dev").await;
    let _org = dev.orgs().create("dev org", None, None).await.unwrap();
    let (carol, mut carol_key) = signed_up(&c, 1, "carol").await;
    let (bob, bob_key) = signed_up(&c, 2, "bob").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[29; 32]);
    let mut mpks = Vec::new();
    for (version, tag) in [("1.0.0", "v1"), ("1.5.0", "v15"), ("2.0.0", "v2")] {
        let (mpk, _) = release_of(
            PACKAGE,
            version,
            &variant(&kv_wasm(), tag),
            &publisher,
            &Declares::default(),
        );
        let _ = dev.packages().upload(mpk.clone(), None).await.unwrap();
        mpks.push(mpk);
    }
    let dev_id = account_of(&dev).await;
    let d = dev
        .deployments()
        .create(
            "acme",
            typed(json!({ "account": dev_id })),
            Some(&mut dev_pk),
        )
        .await
        .map(wire)
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ok = dev
        .deployments()
        .offer(&d, "notes", PACKAGE, "1.0.0", Some(&mut dev_pk))
        .await
        .unwrap();

    // Carol's organization; bob, a member who may create apps, creates the instance.
    let globex = carol
        .orgs()
        .create("Globex", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    // Globex may own a deployment: it holds a recovery key.
    let key = calimero_identity::recovery::recovery_public(&[8; 32]);
    let _ok = carol
        .orgs()
        .recovery_set(&globex, Some(Some(&key)), Some(&mut carol_key))
        .await
        .unwrap();
    let token = carol
        .orgs()
        .invite_create(
            &globex,
            typed(json!({ "role": "member" })),
            Some(&mut carol_key),
        )
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _r = bob.orgs().invite_redeem(&token).await.unwrap();
    let bob_id = account_of(&bob).await;
    let _ok = carol
        .orgs()
        .capabilities(
            &globex,
            CREATE_APP,
            Some(&bob_id),
            None,
            Some(&mut carol_key),
        )
        .await
        .unwrap();
    let inst = bob
        .apps()
        .create(typed(json!({
            "name": "notes", "deployment": d, "package": PACKAGE, "org": globex,
        })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _ok = dev
        .deployments()
        .offer(&d, "notes", PACKAGE, "2.0.0", Some(&mut dev_pk))
        .await
        .unwrap();

    // The developer hands the deployment to Globex, the customer's own organization: the
    // update sheet tells its owner who now chooses its updates, and whom it came from.
    let to_globex = || typed(json!({ "org": globex }));
    let _ok = dev
        .deployments()
        .transfer_offer(&d, to_globex(), Some(&mut dev_pk))
        .await
        .unwrap();
    // The developer's releases are private to it: Globex accepts only once it may use what
    // the deployment offers, else the offer would stop serving its customers.
    let unusable = carol
        .deployments()
        .transfer_accept(&d, to_globex(), Some(&mut carol_key))
        .await
        .unwrap_err();
    assert!(
        matches!(&unusable, calimero_client::Error::Api { status: 409, code, .. } if code == "OFFERS_NOT_USABLE"),
        "{unusable:?}"
    );
    for mpk in mpks {
        let _ = carol.packages().upload(mpk, Some(&globex)).await.unwrap();
    }
    let _ok = carol
        .deployments()
        .transfer_accept(&d, to_globex(), Some(&mut carol_key))
        .await
        .unwrap();

    // In the app: each person signs in on the package's frontend.
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let back = format!("{app_origin}/");
    let carol_app = c.client(1);
    let mut carol_in_app = SoftAuthenticator::with_key(&carol_key, "localhost", &app_origin);
    let _s = carol_app.sign_in(&mut carol_in_app).await.unwrap();
    let bob_app = c.client(2);
    let mut bob_in_app = SoftAuthenticator::with_key(&bob_key, "localhost", &app_origin);
    let _s = bob_app.sign_in(&mut bob_in_app).await.unwrap();

    // The frontend learns there is an update, and who may approve it.
    let u = carol_app
        .transport()
        .json(Request::new(Method::GET, format!("apps/{inst}/update")))
        .await
        .unwrap();
    assert_eq!(u["available"]["version"], "2.0.0", "{u}");
    assert_eq!(u["available"]["source"], "deployment", "{u}");
    assert_eq!(u["mayApprove"], json!(true), "{u}");
    // Names are not unique: the deployment comes with its id and its owner, and whom it
    // came from.
    let named = u["deployment"].clone();
    assert_eq!(
        (&named["id"], &named["name"], &named["owner"]),
        (
            &json!(d),
            &json!("acme"),
            &json!({ "kind": "org", "name": "Globex" })
        ),
        "{u}"
    );
    assert_eq!(
        named["transferred"]["from"],
        json!({ "kind": "account", "name": "dev" }),
        "{u}"
    );

    // A second instance from the offer: the install sheet names the deployment the same way.
    let made = ask(
        &carol_app,
        &back,
        "install",
        json!({ "package": PACKAGE, "name": "notes-2", "deployment": d, "org": globex }),
    )
    .await
    .unwrap();
    let model = Page::new(made["url"].as_str().unwrap()).model().await;
    assert_eq!(model["kind"], "install", "{model}");
    assert_eq!(model["deployment"], named, "{model}");
    let offered_by = json!({
        "title": "Offered by acme, run by the organization Globex.",
        "detail": format!("Deployment {d}"),
    });
    assert_eq!(model["offeredBy"], offered_by, "{model}");
    let u = bob_app
        .transport()
        .json(Request::new(Method::GET, format!("apps/{inst}/update")))
        .await
        .unwrap();
    assert_eq!(u["mayApprove"], json!(false), "{u}");

    // A version the deployment does not offer, and a member, are refused.
    assert_eq!(
        refused(
            &carol_app,
            &back,
            "upgrade",
            json!({ "app": inst, "toVersion": "1.5.0" })
        )
        .await,
        409
    );
    assert_eq!(
        refused(&bob_app, &back, "upgrade", json!({ "app": inst })).await,
        403
    );

    // The offer, named or not: the owner approves it on the node's page.
    let made = ask(
        &carol_app,
        &back,
        "upgrade",
        json!({ "app": inst, "toVersion": "2.0.0" }),
    )
    .await
    .unwrap();
    assert!(made["url"].is_string(), "{made}");
    let made = ask(&carol_app, &back, "upgrade", json!({ "app": inst }))
        .await
        .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["kind"], "upgrade", "{model}");
    assert_eq!(model["upgrade"]["to"]["version"], "2.0.0", "{model}");
    assert_eq!(
        model["deployment"]["owner"],
        json!({ "kind": "org", "name": "Globex" }),
        "{model}"
    );
    assert_eq!(model["offeredBy"], offered_by, "{model}");
    assert_eq!(
        model["takenOver"]["title"], "acme has a new owner",
        "{model}"
    );
    let taken = "Now run by the organization Globex, who took it over from the account dev. They choose this app's updates from now on.";
    assert_eq!(model["takenOver"]["detail"], taken, "{model}");
    let mut carol_page = SoftAuthenticator::with_key(&carol_in_app, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut carol_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__upgrade", "{args}");
    let moved = get_until(&carol, &format!("apps/{inst}"), |a| {
        a["release"]["version"] == "2.0.0"
    })
    .await;
    assert_eq!(moved["deployment"], json!(d), "{moved}");
}

/// Where a unit may send data is computed over the app and every app it created, and
/// only destinations are reported: an empty list means no data leaves Calimero, and an
/// external endpoint, a TEE (here NEAR AI) included, is a destination like any other. The
/// capability card, `GET /apps/{app}/egress`, `GET /apps/{app}/update` (with the
/// classifier's verdict) and the unit's upgrade page all say so.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_units_destinations_are_judged_over_its_members() {
    const AI: &str = "https://inference.near.ai/v1/";
    const TELEMETRY: &str = "https://telemetry.example/";
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[29; 32]);
    let (kv, ai) = ("com.test.unit-kv", "com.test.unit-ai");
    let uses = |kv_version: &str| Declares {
        uses: Some(json!({
            "store": { "package": kv, "range": "^0.1", "version": kv_version },
            "ai": { "package": ai, "range": "^1", "version": "1.0.0" },
        })),
        ..Declares::default()
    };
    // 2.0.0 moves the store to a release that also reports to a telemetry host.
    for (package, version, d) in [
        (kv, "0.1.0", Declares::default()),
        (
            kv,
            "0.1.1",
            Declares {
                outbound: &[TELEMETRY],
                ..Declares::default()
            },
        ),
        (
            ai,
            "1.0.0",
            Declares {
                outbound: &[AI],
                ..Declares::default()
            },
        ),
        (PACKAGE, "1.0.0", uses("0.1.0")),
        (PACKAGE, "2.0.0", uses("0.1.1")),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }

    // A release that declares no destination keeps data in Calimero; NEAR AI is a
    // destination, of its own release and of the release that creates it.
    let store_card = get(&alice, &format!("packages/{kv}/0.1.0/capabilities"))
        .await
        .unwrap();
    assert_eq!(store_card["destinations"], json!([]), "{store_card}");
    assert!(store_card.get("sealed").is_none(), "{store_card}");
    let ai_card = get(&alice, &format!("packages/{ai}/1.0.0/capabilities"))
        .await
        .unwrap();
    assert_eq!(ai_card["destinations"], json!([AI]), "{ai_card}");
    let card = get(&alice, &format!("packages/{PACKAGE}/1.0.0/capabilities"))
        .await
        .unwrap();
    assert_eq!(card["outbound"], json!([]), "{card}");
    assert_eq!(card["destinations"], json!([AI]), "{card}");
    assert_eq!(card["members"]["ai"]["destinations"], json!([AI]), "{card}");
    assert_eq!(card["members"]["store"]["version"], "0.1.0", "{card}");

    // An instance and its unit: data can go to NEAR AI, through its AI member.
    let made = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap();
    let notes = made["appId"].as_str().unwrap().to_owned();
    let egress = get(&alice, &format!("apps/{notes}/egress")).await.unwrap();
    assert_eq!(egress["app"], json!(notes), "{egress}");
    assert_eq!(egress["destinations"], json!([AI]), "{egress}");
    assert!(egress.get("sealed").is_none(), "{egress}");
    assert_eq!(
        egress["members"]["ai"]["app"], made["links"]["ai"],
        "{egress}"
    );
    assert_eq!(
        egress["members"]["store"]["destinations"],
        json!([]),
        "{egress}"
    );
    let (stranger, _) = signed_up(&c, 1, "mallory").await;
    let e = get(&stranger, &format!("apps/{notes}/egress"))
        .await
        .unwrap_err();
    assert_eq!(status(&e), 404, "{e}");

    // The update moves the store to a release that adds a destination: red.
    let update = get(&alice, &format!("apps/{notes}/update")).await.unwrap();
    // From no deployment: named as none, not left out.
    assert_eq!(update.get("deployment"), Some(&json!(null)), "{update}");
    let available = &update["available"];
    assert_eq!(available["version"], "2.0.0", "{update}");
    assert_eq!(available["class"], "red", "{update}");
    assert_eq!(available["reasons"][0]["code"], "egressAdded", "{update}");
    assert_eq!(available["egress"]["added"], json!([TELEMETRY]), "{update}");
    assert_eq!(available["egress"]["before"], json!([AI]), "{update}");
    assert!(available.get("sealedAfter").is_none(), "{update}");
    assert_eq!(available["signerChanged"], false, "{update}");

    // A store on its own keeps data in Calimero until its update adds the telemetry host.
    let store = alice
        .apps()
        .create(typed(
            json!({ "name": "store", "package": kv, "version": "0.1.0" }),
        ))
        .await
        .map(wire)
        .unwrap();
    let store = store["appId"].as_str().unwrap().to_owned();
    let egress = get(&alice, &format!("apps/{store}/egress")).await.unwrap();
    assert_eq!(egress["destinations"], json!([]), "{egress}");
    let update = get(&alice, &format!("apps/{store}/update")).await.unwrap();
    let available = &update["available"];
    assert_eq!(available["class"], "red", "{update}");
    assert_eq!(
        (
            &available["egress"]["before"],
            &available["egress"]["after"]
        ),
        (&json!([]), &json!([TELEMETRY])),
        "{update}"
    );

    // The unit's page says so, before anything moves, naming the apps it created.
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let made = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "upgradeUnit", "subject": { "app": notes, "toVersion": "2.0.0" },
                "returnTo": format!("{app_origin}/1.0.0/") }),
    )
    .await
    .unwrap();
    let model = Page::new(made["url"].as_str().unwrap()).model().await;
    assert_eq!(model["unit"]["class"], "red", "{model}");
    assert_eq!(
        model["unit"]["egress"]["added"],
        json!([TELEMETRY]),
        "{model}"
    );
    let flags: Vec<&str> = model["attention"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["flag"].as_str())
        .collect();
    assert!(flags.contains(&"newDestinations"), "{model}");
    let points: Vec<&str> = model["points"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Value::as_str)
        .collect();
    assert!(
        points.contains(
            &"This update can send data to a new place: telemetry.example. That includes the apps it created."
        ),
        "{model}"
    );
    assert!(
        !points.iter().any(|p| p.to_lowercase().contains("sealed")),
        "{model}"
    );
}

/// An organization's owner sets a policy that asks for a second approver, with a
/// passkey; a red update (a release adding a destination) then waits: the owner's upgrade
/// is refused before any passkey (`409 SECOND_APPROVAL_REQUIRED`), the owner logs the first
/// approval with their own passkey, `GET /apps/{app}/update` and `GET /orgs/{org}/updates`
/// show it waiting, and an admin's upgrade applies it, recorded red with both approvers. A
/// hold outside a day to a week is refused before the passkey; the app page refuses the
/// owner's upgrade before a page exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_red_update_waits_for_a_second_approver() {
    const TELEMETRY: &str = "https://telemetry.example/";
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[31; 32]);
    for (version, outbound) in [("1.0.0", &[][..]), ("2.0.0", &[TELEMETRY][..])] {
        let d = Declares {
            outbound,
            ..Declares::default()
        };
        let (mpk, _) = release(version, &kv_wasm(), &publisher, &d);
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let org = home_org(&alice).await;
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "admin" })), Some(&mut alice_pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = bob.orgs().invite_redeem(&token).await.unwrap();
    let made = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0", "org": org }),
        ))
        .await
        .map(wire)
        .unwrap();
    let notes = made["appId"].as_str().unwrap().to_owned();
    let alice_id = account_of(&alice).await;
    let bob_id = account_of(&bob).await;

    // Nothing updates on its own by default: one approval, the defaults.
    let policy = get(&alice, &format!("apps/{notes}/upgrade-policy"))
        .await
        .unwrap();
    assert_eq!(policy["source"], "default", "{policy}");
    assert_eq!(
        policy["policy"],
        json!({ "autoApply": "never", "holdHours": 24, "secondApprover": false })
    );
    let path = format!("orgs/{org}/upgrade-policy");
    let bad = post(
        &alice,
        &path,
        json!({ "policy": { "secondApprover": true, "holdHours": 23 } }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&bad), 400, "{bad}");
    assert!(bad.to_string().contains("BAD_UPGRADE_POLICY"), "{bad}");
    let set = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, path.as_str())
                .json(json!({ "policy": { "secondApprover": true } })),
            &mut alice_pk,
        )
        .await
        .unwrap();
    assert_eq!(set["source"], "org", "{set}");
    let update = get_until(&alice, &format!("apps/{notes}/update"), |u| {
        u["policy"]["secondApprover"] == true
    })
    .await;
    let available = &update["available"];
    assert_eq!(available["version"], "2.0.0", "{update}");
    assert_eq!(available["class"], "red", "{update}");
    assert_eq!(available["approvalsNeeded"], 2, "{update}");
    assert_eq!(update["pendingApprovals"], json!([]), "{update}");

    // The owner approves first, with their own passkey; the update shows it waiting. The
    // answer is the shape `pendingApprovals` lists, read through the typed client.
    let approvals = format!("apps/{notes}/upgrade-approvals");
    let first = alice
        .apps()
        .approve_upgrade(&notes, "2.0.0", None, Some(&mut alice_pk))
        .await
        .unwrap();
    let approval = &first.approval;
    assert_eq!(
        (approval.app.as_str(), approval.by.as_str()),
        (notes.as_str(), alice_id.as_str()),
        "{first:?}"
    );
    assert_eq!(
        approval.class,
        calimero_client::types::PendingApprovalClass::Red,
        "{first:?}"
    );
    assert_eq!(
        approval.reasons.len(),
        1,
        "the destination it adds: {first:?}"
    );
    assert!(approval.targets.is_empty(), "{first:?}");
    assert_eq!(
        approval.expires_at,
        approval.at + calimero_node::types::MAX_APPROVAL_AGE_SECS,
        "{first:?}"
    );
    assert!(
        approval.extra.is_empty(),
        "nothing the schema leaves out: {first:?}"
    );
    // The owner alone: refused before any passkey, here and on the app's page.
    let e = alice
        .apps()
        .upgrade(
            &notes,
            typed(json!({ "version": "2.0.0" })),
            Some(&mut alice_pk),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("SECOND_APPROVAL_REQUIRED"), "{e}");
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(0);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_pk, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let refused = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "upgrade", "subject": { "app": notes, "toVersion": "2.0.0" },
                "returnTo": format!("{app_origin}/1.0.0/") }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&refused), 409, "{refused}");
    assert!(
        refused.to_string().contains("SECOND_APPROVAL_REQUIRED"),
        "{refused}"
    );

    let again = post(&bob, &approvals, json!({ "version": "2.0.0" }))
        .await
        .unwrap_err();
    assert!(again.to_string().contains("APPROVAL_PENDING"), "{again}");
    let update = get_until(&bob, &format!("apps/{notes}/update"), |u| {
        u["pendingApprovals"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
    })
    .await;
    assert_eq!(update["pendingApprovals"][0]["by"], json!(alice_id));
    assert_eq!(update["available"]["approvedBy"], json!(alice_id));
    let listed = get(&bob, &format!("orgs/{org}/updates")).await.unwrap();
    assert_eq!(listed["updates"][0]["app"], json!(notes), "{listed}");
    assert_eq!(listed["updates"][0]["name"], "notes", "{listed}");
    let e = alice
        .apps()
        .upgrade(
            &notes,
            typed(json!({ "version": "2.0.0" })),
            Some(&mut alice_pk),
        )
        .await
        .unwrap_err();
    assert_eq!(
        e.code(),
        Some("SECOND_APPROVAL_REQUIRED"),
        "not the same person twice: {e}"
    );

    // The admin applies it: recorded red, with the first approver.
    let _done = bob
        .apps()
        .upgrade(
            &notes,
            typed(json!({ "version": "2.0.0" })),
            Some(&mut bob_pk),
        )
        .await
        .unwrap();
    let moved = get_until(&alice, &format!("apps/{notes}"), |a| {
        a["release"]["version"] == "2.0.0"
    })
    .await;
    let last = &moved["lastUpgrade"];
    assert_eq!(last["class"], "red", "{moved}");
    assert_eq!(last["reasons"][0]["code"], "egressAdded", "{moved}");
    assert_eq!(last["by"], json!(bob_id), "{moved}");
    assert_eq!(last["approvedBy"], json!(alice_id), "{moved}");
    let update = get(&alice, &format!("apps/{notes}/update")).await.unwrap();
    assert_eq!(update["pendingApprovals"], json!([]), "spent: {update}");
}

/// The install sheet: an app's frontend asks to create an instance of its release, and the node's
/// install sheet shows what gets created (the app and the apps its release creates, with their
/// versions), where data can go over the whole unit, and that updates wait for an approval.
/// The person's passkey on the page creates it as `POST /apps` does, and the app reads the new
/// instance from `/result`. A frontend may not ask for another package's instance.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_install_sheet_shows_what_gets_created_and_where_data_can_go() {
    const AI: &str = "https://inference.near.ai/v1/";
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[37; 32]);
    let (kv, ai) = ("com.test.unit-kv", "com.test.unit-ai");
    for (package, version, d) in [
        (kv, "0.1.0", Declares::default()),
        (
            ai,
            "1.0.0",
            Declares {
                outbound: &[AI],
                ..Declares::default()
            },
        ),
        (
            PACKAGE,
            "1.0.0",
            Declares {
                uses: Some(json!({
                    "store": { "package": kv, "range": "^0.1", "version": "0.1.0" },
                    "ai": { "package": ai, "range": "^1", "version": "1.0.0" },
                })),
                ..Declares::default()
            },
        ),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let org = home_org(&alice).await;
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let back = format!("{app_origin}/1.0.0/");
    let in_app = c.client(1);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();

    // Another package's instance: not this frontend's to ask for.
    let other = refused(
        &in_app,
        &back,
        "install",
        json!({ "package": kv, "version": "0.1.0", "name": "store" }),
    )
    .await;
    assert_eq!(other, 403);

    let made = ask(
        &in_app,
        &back,
        "install",
        json!({ "package": PACKAGE, "version": "1.0.0", "name": "notes", "org": org }),
    )
    .await
    .unwrap();
    let id = made["requestId"].as_str().unwrap().to_owned();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["kind"], "install", "{model}");
    assert_eq!(model["title"], "Start using Notes", "{model}");
    let install = &model["install"];
    let creates: Vec<(&str, &str)> = install["creates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["package"].as_str().unwrap(),
                c["version"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        creates,
        vec![(PACKAGE, "1.0.0"), (ai, "1.0.0"), (kv, "0.1.0")],
        "{model}"
    );
    assert_eq!(
        install["destinations"],
        json!(["inference.near.ai"]),
        "{model}"
    );
    assert_eq!(
        install["where"], "This app can send data to: inference.near.ai.",
        "{model}"
    );
    assert_eq!(
        install["updates"][0], "You approve each update. Nothing changes until you do.",
        "{model}"
    );
    assert_eq!(install["policy"]["source"], "default", "{model}");
    let text = model.to_string().to_lowercase();
    assert!(!text.contains("sealed"), "{model}");

    let mut alice_page = SoftAuthenticator::with_key(&alice_in_app, "localhost", &origin(&url));
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__create_instance", "{args}");
    assert_eq!(args["package"], PACKAGE, "{args}");
    assert_eq!(args["version"], "1.0.0", "{args}");
    let result = in_app.get(&format!("consent/{id}/result")).await.unwrap();
    assert_eq!(result["status"], "approved", "{result}");
    assert_eq!(result["kind"], "install", "{result}");
    let notes = result["outcome"]["appId"].as_str().unwrap().to_owned();
    assert!(result["outcome"]["links"]["store"].is_string(), "{result}");
    let made = get_until(&alice, &format!("apps/{notes}"), |a| {
        a["release"]["version"] == "1.0.0"
    })
    .await;
    assert_eq!(made["org"], json!(org), "{made}");
    let egress = get(&alice, &format!("apps/{notes}/egress")).await.unwrap();
    assert_eq!(egress["destinations"], json!([AI]), "{egress}");
}

/// A person in Acme and Globex installs from a frontend that names no organization. A release
/// private to Acme alone is created in Acme with no choice, as a named `org`; one private to
/// both, or the operator's catalog, offers both by name, and the choice reaches the install as
/// `org` under the passkey; no choice, or one they are not in, is refused before the passkey.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_install_sheet_offers_the_organizations_a_release_is_for() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let org = |v: Value| v["orgId"].as_str().unwrap().to_owned();
    let acme = org(alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap());
    let globex = org(alice
        .orgs()
        .create("Globex", None, None)
        .await
        .map(wire)
        .unwrap());
    let bobco = org(bob
        .orgs()
        .create("Bobco", None, None)
        .await
        .map(wire)
        .unwrap());
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[38; 32]);
    let release = |package: &str| {
        release_of(
            package,
            "1.0.0",
            &kv_wasm(),
            &publisher,
            &Declares::default(),
        )
        .0
    };
    let (acme_only, both, catalog) = ("com.test.acme", "com.test.both", "com.test.catalog");
    let _ = alice
        .packages()
        .upload(release(acme_only), Some(acme.as_str()))
        .await
        .unwrap();
    for o in [&acme, &globex] {
        let _ = alice
            .packages()
            .upload(release(both), Some(o.as_str()))
            .await
            .unwrap();
    }
    let _ = ops.ops().package_upload(release(catalog)).await.unwrap();

    // Each sheet's passkey goes on from the last one's counter.
    let (page, mut key) = install_sheet_naming_no_org(&c, &alice_key, acme_only).await;
    let model = page.model().await;
    let install = &model["install"];
    assert_eq!(install["org"], json!(acme), "{model}");
    assert_eq!(install["orgName"], "Acme", "{model}");
    assert!(install["chooseOrg"].is_null(), "{model}");
    assert_eq!(install["privateTo"], "Private to Acme", "{model}");
    assert_eq!(
        model["points"][0], "notes is created for you in Acme: Notes 1.0.0.",
        "{model}"
    );
    let (code, refused) = page.post("approve", json!({ "org": globex })).await;
    assert_eq!(code, 400, "nothing to choose: {refused}");
    let ((code, done), args) = page.approve(&c, &mut key).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["org"], json!(acme), "{args}");
    let app = done["result"]["appId"].as_str().unwrap();
    let made = get(&alice, &format!("apps/{app}")).await.unwrap();
    assert_eq!(made["org"], json!(acme), "{made}");

    let offered = json!([
        { "orgId": acme, "name": "Acme", "point": "notes is created for you in Acme: Notes 1.0.0." },
        { "orgId": globex, "name": "Globex", "point": "notes is created for you in Globex: Notes 1.0.0." },
    ]);
    for (package, private_to) in [
        (both, json!("Private to Acme and Globex")),
        (catalog, Value::Null),
    ] {
        let (page, next) = install_sheet_naming_no_org(&c, &key, package).await;
        key = next;
        let model = page.model().await;
        assert_eq!(model["install"]["chooseOrg"], offered, "{package}: {model}");
        assert!(model["install"]["org"].is_null(), "{package}: {model}");
        assert_eq!(
            model["install"]["privateTo"], private_to,
            "{package}: {model}"
        );
        for (body, code) in [
            (json!({}), "ORG_REQUIRED"),
            (json!({ "org": bobco }), "NOT_FOUND"),
        ] {
            let (status, refused) = page.post("approve", body.clone()).await;
            assert_eq!(
                refused["error"], code,
                "{package} {body}: {status} {refused}"
            );
        }
        let ((code, done), args) = page
            .approve_with(&c, &mut key, json!({ "org": globex }))
            .await;
        assert_eq!(code, 200, "{package}: {done}");
        assert_eq!(
            args["org"],
            json!(globex),
            "the passkey signs the choice: {args}"
        );
        let app = done["result"]["appId"].as_str().unwrap();
        let made = get(&alice, &format!("apps/{app}")).await.unwrap();
        assert_eq!(made["org"], json!(globex), "{package}: {made}");
    }
}

/// An install sheet for `package`, asked for by its frontend signed in as `key`'s account
/// and naming no organization; with the passkey on the page's origin.
async fn install_sheet_naming_no_org(
    c: &Cluster,
    key: &SoftAuthenticator,
    package: &str,
) -> (Page, SoftAuthenticator) {
    let port = url::Url::parse(&c.urls[1]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(package)
    );
    let in_app = c.client(1);
    let mut in_app_passkey = SoftAuthenticator::with_key(key, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut in_app_passkey).await.unwrap();
    let made = ask(
        &in_app,
        &format!("{app_origin}/1.0.0/"),
        "install",
        json!({ "package": package, "version": "1.0.0", "name": "notes" }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page_key = SoftAuthenticator::with_key(&in_app_passkey, "localhost", &origin(&url));
    (Page::new(&url), page_key)
}

/// The update sheets: an update's sheet says its class in plain words. A green one
/// (nothing changes where data goes) is "Update with passkey" / "Not now"; a red one adding a
/// destination names it and leads with keeping the current version. Once the organization's
/// policy (set on the page) asks for a second approver, the owner's red sheet is "Approve (1 of
/// 2)" and logs the first approval; an admin unlocks the node's updates page with a passkey,
/// sees the update waiting with its class and the policy, opens its sheet from there, and
/// "Approve and update (2 of 2)" applies it. The updates page lists nothing before it is
/// unlocked, and stays open while the sheets it opens come and go.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_update_shows_its_class_and_takes_two_approvers_on_the_pages() {
    const TELEMETRY: &str = "https://telemetry.example/";
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let (bob, bob_key) = signed_up(&c, 1, "bob").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[41; 32]);
    for (version, outbound) in [
        ("1.0.0", &[][..]),
        ("1.0.1", &[][..]),
        ("2.0.0", &[TELEMETRY][..]),
    ] {
        let d = Declares {
            outbound,
            ..Declares::default()
        };
        let (mpk, _) = release(version, &kv_wasm(), &publisher, &d);
        let _ = alice
            .packages()
            .upload(mpk, Some(&home_org(&alice).await))
            .await
            .unwrap();
    }
    let org = home_org(&alice).await;
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "admin" })), Some(&mut alice_pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = bob.orgs().invite_redeem(&token).await.unwrap();
    let made = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0", "org": org }),
        ))
        .await
        .map(wire)
        .unwrap();
    let notes = made["appId"].as_str().unwrap().to_owned();
    let alice_id = account_of(&alice).await;
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let back = format!("{app_origin}/1.0.0/");
    let in_app = c.client(0);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_pk, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    // One authenticator per person on the node's page, so its counter moves on.
    let node_origin = origin(&c.urls[0]);
    let mut alice_page = SoftAuthenticator::with_key(&alice_in_app, "localhost", &node_origin);

    // The banner's line: `GET /apps/{app}/update` says it in the page's words.
    let update = get(&in_app, &format!("apps/{notes}/update")).await.unwrap();
    assert_eq!(
        update["available"]["summary"],
        "This update can send data to a new place: telemetry.example",
        "{update}"
    );

    // Green: nothing changes where data goes.
    let made = ask(
        &in_app,
        &back,
        "upgrade",
        json!({ "app": notes, "toVersion": "1.0.1" }),
    )
    .await
    .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let u = page.model().await["update"].clone();
    assert_eq!(u["class"], "green", "{u}");
    assert_eq!(u["summary"], "Nothing changes where your data goes", "{u}");
    assert_eq!(
        u["destinations"]["words"], "Unchanged: your data stays in Calimero",
        "{u}"
    );
    assert_eq!(u["approveLabel"], "Update with passkey", "{u}");
    assert_eq!(u["keepLabel"], "Not now", "{u}");
    assert_eq!(u["keepByDefault"], false, "{u}");
    assert_eq!(u["approval"]["step"], "only", "{u}");
    let (code, _) = page.post("decline", json!({})).await;
    assert_eq!(code, 200);

    // Red: a new destination, named, and keeping the current version leads.
    let made = ask(
        &in_app,
        &back,
        "upgrade",
        json!({ "app": notes, "toVersion": "2.0.0" }),
    )
    .await
    .unwrap();
    let page = Page::new(made["url"].as_str().unwrap());
    let u = page.model().await["update"].clone();
    assert_eq!(u["class"], "red", "{u}");
    assert_eq!(
        u["reasons"][0], "This update can send data to a new place: telemetry.example.",
        "{u}"
    );
    assert_eq!(
        u["destinations"]["added"],
        json!(["telemetry.example"]),
        "{u}"
    );
    assert_eq!(u["keepByDefault"], true, "{u}");
    assert_eq!(u["keepLabel"], "Keep 1.0.0", "{u}");
    assert_eq!(u["approveLabel"], "Approve with passkey", "{u}");
    assert_eq!(
        u["policy"]["words"][0], "You approve each update. Nothing changes until you do.",
        "{u}"
    );
    let (code, _) = page.post("decline", json!({})).await;
    assert_eq!(code, 200);

    // The organization's policy, changed on the page: a second approver for red updates.
    let made = ask(
        &in_app,
        &back,
        "updatePolicy",
        json!({ "org": org, "policy": { "secondApprover": true } }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    assert_eq!(model["kind"], "updatePolicy", "{model}");
    assert!(
        model["policy"]["afterWords"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("two owners or admins")),
        "{model}"
    );
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__set_upgrade_policy", "{args}");
    let _set = get_until(&alice, &format!("apps/{notes}/update"), |u| {
        u["policy"]["secondApprover"] == true
    })
    .await;

    // The owner's red sheet is now the first of two approvals.
    let made = ask(
        &in_app,
        &back,
        "upgrade",
        json!({ "app": notes, "toVersion": "2.0.0" }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let u = page.model().await["update"].clone();
    assert_eq!(u["approval"]["step"], "first", "{u}");
    assert_eq!(u["approveLabel"], "Approve (1 of 2)", "{u}");
    let ((code, done), args) = page.approve(&c, &mut alice_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__approve_upgrade", "{args}");
    let _first = get_until(&alice, &format!("apps/{notes}/update"), |u| {
        u["available"]["approvedBy"] == json!(alice_id)
    })
    .await;
    // Not the same person twice.
    let again = refused(
        &in_app,
        &back,
        "upgrade",
        json!({ "app": notes, "toVersion": "2.0.0" }),
    )
    .await;
    assert_eq!(again, 409);

    // The admin's updates page: locked until their passkey, then the update waits there.
    let bob_in_app = c.client(0);
    let mut bob_in_app_passkey = SoftAuthenticator::with_key(&bob_key, "localhost", &app_origin);
    let _s = bob_in_app.sign_in(&mut bob_in_app_passkey).await.unwrap();
    let made = ask(&bob_in_app, &back, "updates", json!({})).await.unwrap();
    let hub_url = made["url"].as_str().unwrap().to_owned();
    assert!(hub_url.contains("/updates/"), "{made}");
    let served = reqwest::get(&hub_url).await.unwrap();
    assert_eq!(served.status(), 200);
    let hub = Page::new(&hub_url.replace("/updates/", "/consent/"));
    let locked = hub.model().await;
    assert_eq!(locked["kind"], "updates", "{locked}");
    assert_eq!(locked["locked"], true, "{locked}");
    assert!(locked.get("updates").is_none(), "{locked}");
    let (code, early) = hub
        .post(
            "open",
            json!({ "kind": "upgrade", "subject": { "app": notes, "toVersion": "2.0.0" } }),
        )
        .await;
    assert_eq!(code, 409, "{early}");
    let mut bob_page = SoftAuthenticator::with_key(&bob_in_app_passkey, "localhost", &node_origin);
    let ((code, done), args) = hub.approve(&c, &mut bob_page).await;
    assert_eq!(code, 200, "{done}");
    assert_eq!(args["__method"], "__view_updates", "{args}");
    let open = hub.model().await;
    assert_eq!(open["locked"], false, "{open}");
    let row = &open["updates"]["pending"][0];
    assert_eq!(row["app"], json!(notes), "{open}");
    assert_eq!(row["class"], "red", "{open}");
    assert_eq!(
        (&row["from"], &row["to"]),
        (&json!("1.0.0"), &json!("2.0.0"))
    );
    assert_eq!(row["approvedBy"], json!(alice_id), "{open}");
    assert_eq!(
        row["summary"], "This update can send data to a new place: telemetry.example",
        "{open}"
    );
    assert_eq!(
        open["updates"]["policies"][0]["policy"]["secondApprover"], true,
        "{open}"
    );

    // From the updates page, the update's sheet: the second approval applies it.
    let (code, child) = hub
        .post(
            "open",
            json!({ "kind": "upgrade", "subject": { "app": notes, "toVersion": "2.0.0" } }),
        )
        .await;
    assert_eq!(code, 200, "{child}");
    let url = child["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let model = page.model().await;
    let u = &model["update"];
    assert_eq!(u["approval"]["step"], "second", "{u}");
    assert_eq!(u["approveLabel"], "Approve and update (2 of 2)", "{u}");
    assert_eq!(
        model["requestedBy"]["returnOrigin"],
        json!(origin(&hub_url)),
        "{model}"
    );
    let ((code, done), _args) = page.approve(&c, &mut bob_page).await;
    assert_eq!(code, 200, "{done}");
    assert!(
        done["redirect"].as_str().unwrap().contains("/updates/"),
        "{done}"
    );
    let moved = get_until(&alice, &format!("apps/{notes}"), |a| {
        a["release"]["version"] == "2.0.0"
    })
    .await;
    assert_eq!(
        moved["lastUpgrade"]["approvedBy"],
        json!(alice_id),
        "{moved}"
    );
    // The updates page outlived the sheet it opened, and has nothing left to show.
    let after = hub.model().await;
    assert_eq!(after["updates"]["pending"], json!([]), "{after}");
}

/// A unit red only through its member: alice's organization holds `notes` on
/// `PACKAGE@1.0.0`, which created `store` on `com.test.c8-kv@0.1.0`. `PACKAGE@2.0.0` ships the same WASM and
/// destinations (its own move is green) but takes the store to `0.1.1`, which adds a
/// telemetry host: the unit's update is red only through its member. Answers the
/// organization, `notes` and `store`.
async fn unit_red_through_its_member(
    alice: &calimero_client::Client,
    seed: u8,
) -> (String, String, String) {
    const TELEMETRY: &str = "https://telemetry.example/";
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    let kv = "com.test.c8-kv";
    let uses = |kv_version: &str| Declares {
        uses: Some(json!({ "store": { "package": kv, "range": "^0.1", "version": kv_version } })),
        ..Declares::default()
    };
    for (package, version, d) in [
        (kv, "0.1.0", Declares::default()),
        (
            kv,
            "0.1.1",
            Declares {
                outbound: &[TELEMETRY],
                ..Declares::default()
            },
        ),
        (PACKAGE, "1.0.0", uses("0.1.0")),
        (PACKAGE, "2.0.0", uses("0.1.1")),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = alice
            .packages()
            .upload(mpk, Some(&home_org(alice).await))
            .await
            .unwrap();
    }
    let org = home_org(alice).await;
    let made = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0", "org": org }),
        ))
        .await
        .map(wire)
        .unwrap();
    let notes = made["appId"].as_str().unwrap().to_owned();
    let store = made["links"]["store"].as_str().unwrap().to_owned();
    let update = get(alice, &format!("apps/{notes}/update")).await.unwrap();
    assert_eq!(update["available"]["class"], "red", "{update}");
    (org, notes, store)
}

/// A unit's page is judged on the unit, not its instance alone. A unit red
/// only because a member adds a destination, in an organization whose policy asks for a
/// second approver, gets a page whose step is the first of two approvals, as `upgrade-unit`
/// needs one; the instance's own (green) move needs one approval.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_unit_red_through_a_member_is_approved_first_on_its_page() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (org, notes, _store) = unit_red_through_its_member(&alice, 37).await;
    let _set = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/upgrade-policy").as_str())
                .json(json!({ "policy": { "secondApprover": true } })),
            &mut alice_pk,
        )
        .await
        .unwrap();
    let _seen = get_until(&alice, &format!("apps/{notes}/update"), |u| {
        u["policy"]["secondApprover"] == true
    })
    .await;
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(0);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_pk, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let ask = |kind: &str| {
        json!({ "kind": kind, "subject": { "app": notes, "toVersion": "2.0.0" },
                "returnTo": format!("{app_origin}/1.0.0/") })
    };
    // The instance alone: green, one approval, a page.
    let alone = post(&in_app, "consent/requests", ask("upgrade"))
        .await
        .unwrap();
    let model = Page::new(alone["url"].as_str().unwrap()).model().await;
    assert_eq!(model["unit"]["class"], "green", "{model}");
    assert_eq!(model["update"]["approval"]["step"], "only", "{model}");
    // The unit: red through its member, so its page asks for the first of two approvals.
    let unit = post(&in_app, "consent/requests", ask("upgradeUnit"))
        .await
        .unwrap();
    let model = Page::new(unit["url"].as_str().unwrap()).model().await;
    assert_eq!(model["update"]["class"], "red", "{model}");
    assert_eq!(model["update"]["approval"]["step"], "first", "{model}");
    assert_eq!(model["update"]["approval"]["needed"], 2, "{model}");
}

/// A unit whose member's release is uninstalled, under a `secondApprover` policy: alice's
/// organization holds `notes` on `PACKAGE@1.0.0`, which created `store` on
/// `com.test.c8-kv@0.1.0`; `PACKAGE@2.0.0` no longer creates it, so the store stays where it
/// is and the move is green while the log holds its release. Then `0.1.0` is uninstalled
/// (`0.1.1` still ships its WASM), and the move cannot be classified. Answers the
/// organization and `notes`.
async fn unit_with_an_uninstalled_member(
    alice: &calimero_client::Client,
    alice_pk: &mut SoftAuthenticator,
    seed: u8,
) -> (String, String) {
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    let kv = "com.test.c8-kv";
    // 2.0.0 no longer creates the store, which therefore stays where it is: the move is
    // green while the log holds the store's release.
    for (package, version, d) in [
        (kv, "0.1.0", Declares::default()),
        (kv, "0.1.1", Declares::default()),
        (
            PACKAGE,
            "1.0.0",
            Declares {
                uses: Some(
                    json!({ "store": { "package": kv, "range": "^0.1", "version": "0.1.0" } }),
                ),
                ..Declares::default()
            },
        ),
        (PACKAGE, "2.0.0", Declares::default()),
    ] {
        let (mpk, _) = release_of(package, version, &kv_wasm(), &publisher, &d);
        let _ = alice
            .packages()
            .upload(mpk, Some(&home_org(alice).await))
            .await
            .unwrap();
    }
    let org = home_org(alice).await;
    let made = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0", "org": org }),
        ))
        .await
        .map(wire)
        .unwrap();
    let notes = made["appId"].as_str().unwrap().to_owned();
    let update = get(alice, &format!("apps/{notes}/update")).await.unwrap();
    assert_eq!(update["available"]["class"], "green", "{update}");
    let _set = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/upgrade-policy").as_str())
                .json(json!({ "policy": { "secondApprover": true } })),
            alice_pk,
        )
        .await
        .unwrap();
    // The store's release goes; 0.1.1 still ships its WASM.
    let _gone = alice
        .packages()
        .version_uninstall(kv, "0.1.0")
        .await
        .unwrap();
    (org, notes)
}

/// A unit whose member runs a release no longer installed (uninstalled while another
/// release ships its WASM) cannot be classified from the log, so its update is shown as
/// it is gated and logged: red, with the `unclassified` reason naming the member's
/// release, in `GET /apps/{app}/update` and on the unit's page, whose step under a
/// `secondApprover` policy is the first of two. Before, the shown verdict dropped the
/// member it could not read and said green while the upgrade waited for a second approver.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_unit_whose_members_release_is_uninstalled_is_shown_red_unclassified() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (_org, notes) = unit_with_an_uninstalled_member(&alice, &mut alice_pk, 47).await;
    let kv = "com.test.c8-kv";
    let unclassified =
        json!([{ "code": "unclassified", "member": "store", "package": kv, "version": "0.1.0" }]);
    let update = get_until(&alice, &format!("apps/{notes}/update"), |u| {
        u["policy"]["secondApprover"] == true
    })
    .await;
    assert_eq!(update["available"]["class"], "red", "{update}");
    assert_eq!(update["available"]["reasons"], unclassified, "{update}");
    assert_eq!(update["available"]["approvalsNeeded"], 2, "{update}");
    // What depends on the side the log lacks is unknown, never "unchanged".
    const UNKNOWN: [&str; 5] = [
        "egress",
        "stateVersionChanged",
        "newMethods",
        "removedMethods",
        "signerChanged",
    ];
    for fact in UNKNOWN {
        assert_eq!(
            update["available"].get(fact),
            Some(&Value::Null),
            "{fact}: {update}"
        );
    }

    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(0);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_pk, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let unit = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "upgradeUnit", "subject": { "app": notes, "toVersion": "2.0.0" },
                "returnTo": format!("{app_origin}/1.0.0/") }),
    )
    .await
    .unwrap();
    let model = Page::new(unit["url"].as_str().unwrap()).model().await;
    let shown = &model["update"];
    assert_eq!(shown["class"], "red", "{model}");
    assert_eq!(shown["approval"]["step"], "first", "{model}");
    assert_eq!(shown["approval"]["needed"], 2, "{model}");
    assert!(
        shown["reasons"][0]
            .as_str()
            .is_some_and(|r| r.contains("no longer installed")),
        "{model}"
    );
    assert_eq!(model["unit"]["reasons"], unclassified, "{model}");
    for fact in UNKNOWN {
        assert_eq!(
            model["unit"].get(fact),
            Some(&Value::Null),
            "{fact}: {model}"
        );
    }
    for fact in ["newMethods", "removedMethods", "destinations"] {
        assert_eq!(shown.get(fact), Some(&Value::Null), "{fact}: {model}");
    }
    for fact in ["signer", "stateVersion", "actsAs"] {
        let words = shown[fact].as_str().unwrap_or_default();
        assert!(
            words.starts_with("Unknown") && !words.contains("Same") && !words.contains("Unchanged"),
            "{fact}: {model}"
        );
    }
}

/// An update the log cannot classify is approved and logged with its `unclassified`
/// reason only from `Feature::UnclassifiedUpdates` (1.0.0-rc.35): a node on rc.34 cannot
/// decode it and would stop its system group on it. While one node still advertises
/// rc.34, its first approval, its upgrade and its consent page are refused `409
/// FEATURE_NOT_ACTIVE` before any passkey (the client sent none), saying every node must
/// run rc.35 first; the update itself still shows red. Once every node runs rc.35 the
/// same approval and upgrade go through.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_unclassified_update_waits_for_every_node_on_the_release_that_decodes_it() {
    const RC34: &str = "1.0.0-rc.34";
    const RC35: &str = "1.0.0-rc.35";
    // Node 3 advertises rc.34, as a node not yet rolled onto this release does.
    let c = Cluster::start_with(3, |cfg| {
        cfg.apps_domain = Some("localhost".into());
        if cfg.node_id == 3 {
            calimero_node::testing::advertise_cluster_version(&cfg.public_url, Some(RC34));
        }
    })
    .await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let (org, notes) = unit_with_an_uninstalled_member(&alice, &mut alice_pk, 53).await;
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "admin" })), Some(&mut alice_pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = bob.orgs().invite_redeem(&token).await.unwrap();
    let update = get_until(&bob, &format!("apps/{notes}/update"), |u| {
        u["policy"]["secondApprover"] == true
    })
    .await;
    assert_eq!(update["available"]["class"], "red", "{update}");
    assert_eq!(
        update["available"]["reasons"][0]["code"], "unclassified",
        "shown red while it cannot be approved yet: {update}"
    );

    // Below rc.35: refused before any passkey (none is sent, so a step-up would answer 412).
    let not_active = |e: &calimero_client::Error, what: &str| {
        assert_eq!(e.code(), Some("FEATURE_NOT_ACTIVE"), "{what}: {e}");
        let d = e.details().unwrap();
        assert_eq!(d["feature"], "unclassifiedUpdates", "{what}: {e}");
        assert_eq!(d["needs"], RC35, "{what}: {e}");
        assert!(
            e.to_string()
                .contains(&format!("every node must run {RC35} or newer first")),
            "{what}: {e}"
        );
    };
    let e = alice
        .apps()
        .approve_upgrade(&notes, "2.0.0", None, None)
        .await
        .unwrap_err();
    not_active(&e, "the first approval");
    let e = bob
        .apps()
        .upgrade(&notes, typed(json!({ "version": "2.0.0" })), None)
        .await
        .unwrap_err();
    not_active(&e, "the upgrade");
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let app_origin = format!(
        "http://{}.localhost:{port}",
        calimero_node::app_host::label(PACKAGE)
    );
    let in_app = c.client(0);
    let mut alice_in_app = SoftAuthenticator::with_key(&alice_pk, "localhost", &app_origin);
    let _s = in_app.sign_in(&mut alice_in_app).await.unwrap();
    let e = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "upgrade", "subject": { "app": notes, "toVersion": "2.0.0" },
                "returnTo": format!("{app_origin}/1.0.0/") }),
    )
    .await
    .unwrap_err();
    assert_eq!(status(&e), 409, "the consent page: {e}");
    assert!(e.to_string().contains("FEATURE_NOT_ACTIVE"), "{e}");
    let logged = c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.cluster_version.clone());
    assert_eq!(logged.as_deref(), Some(RC34), "held below rc.35");

    // Node 3 runs rc.35 now: the same approval and upgrade go through, logged red.
    calimero_node::testing::advertise_cluster_version(&c.urls[2], None);
    let mut alice_key = SoftAuthenticator::with_key(&alice_in_app, "localhost", ORIGIN);
    let first = alice
        .apps()
        .approve_upgrade(&notes, "2.0.0", None, Some(&mut alice_key))
        .await
        .unwrap();
    assert_eq!(
        wire(&first.approval.reasons)[0]["code"],
        "unclassified",
        "{first:?}"
    );
    let _done = bob
        .apps()
        .upgrade(
            &notes,
            typed(json!({ "version": "2.0.0" })),
            Some(&mut bob_pk),
        )
        .await
        .unwrap();
    let moved = get_until(&alice, &format!("apps/{notes}"), |a| {
        a["release"]["version"] == "2.0.0"
    })
    .await;
    assert_eq!(moved["lastUpgrade"]["class"], "red", "{moved}");
    assert_eq!(
        moved["lastUpgrade"]["reasons"][0]["code"], "unclassified",
        "{moved}"
    );
}

/// A unit's run is judged once, under its instance's policy, so it moves whole. A
/// member whose own policy asks for a second approver, in a unit whose instance's does
/// not, moves with the unit on its one approval and is never refused at apply after the
/// instance has moved, which would leave the unit half upgraded; upgraded alone, the
/// member's own policy still holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_unit_moves_whole_under_its_instances_policy() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (_org, notes, store) = unit_red_through_its_member(&alice, 43).await;
    let _set = alice
        .transport()
        .confirmed(
            Request::new(
                Method::POST,
                format!("apps/{store}/upgrade-policy").as_str(),
            )
            .json(json!({ "policy": { "secondApprover": true } })),
            &mut alice_pk,
        )
        .await
        .unwrap();
    let _seen = get_until(&alice, &format!("apps/{store}/update"), |u| {
        u["policy"]["secondApprover"] == true
    })
    .await;
    // Alone, the member waits for a second approver.
    let e = alice
        .apps()
        .upgrade(
            &store,
            typed(json!({ "version": "0.1.1" })),
            Some(&mut alice_pk),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("SECOND_APPROVAL_REQUIRED"), "{e}");
    // With its unit, it moves under the instance's policy: the whole unit or nothing.
    let _run = alice
        .apps()
        .upgrade_unit(&notes, "2.0.0", Some(&mut alice_pk))
        .await
        .unwrap();
    let member = get_until(&alice, &format!("apps/{store}"), |a| {
        a["lastUpgrade"].is_object()
    })
    .await;
    assert_eq!(member["lastUpgrade"]["ok"], true, "{member}");
    assert_eq!(member["release"]["version"], "0.1.1", "{member}");
    let instance = get(&alice, &format!("apps/{notes}")).await.unwrap();
    assert_eq!(instance["release"]["version"], "2.0.0", "{instance}");
    // The instance's record carries the unit's class: red, the member's destination.
    assert_eq!(instance["lastUpgrade"]["class"], "red", "{instance}");
}

/// The step-up right after a sign-in on an app's own origin (a frontend served at
/// `<label>.<apps_domain>`, its passkey asserting there): the `412 CONFIRM_REQUIRED` a
/// confirm method answers, then its retry with the assertion, wherever each lands. The
/// challenge lives on the node that began it (`{node}.{random}`) and any node takes it
/// from there, so the retry is never `UNKNOWN_CHALLENGE`: on the node the session was made
/// on, on the node that asked, or on another; through the typed client in one go; and an
/// upgrade approved on the consent page the frontend opens through another node. (The
/// reported `UNKNOWN_CHALLENGE` did not reproduce on 1, 3 or 5 nodes; this is its coverage.)
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_step_up_after_an_app_origin_sign_in_is_answered_on_any_node() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let n = c.urls.len();
    let (alice, alice_key) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[41; 32]);
    for version in ["1.0.0", "2.0.0"] {
        let (mpk, _) = release(version, &kv_wasm(), &publisher, &Declares::default());
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let app_origin = |i: usize| {
        let port = url::Url::parse(&c.urls[i]).unwrap().port().unwrap();
        format!(
            "http://{}.localhost:{port}",
            calimero_node::app_host::label(PACKAGE)
        )
    };
    let clear = || {
        Request::new(Method::POST, format!("apps/{app}/intents"))
            .json(json!({ "method": "clear", "args": {} }))
    };
    // One passkey throughout, so its signature counter only moves forward.
    let mut key = alice_key;
    for signed_in_at in 0..n {
        let mut in_app_pk =
            SoftAuthenticator::with_key(&key, "localhost", &app_origin(signed_in_at));
        let in_app = c.client(signed_in_at);
        let session = in_app.sign_in(&mut in_app_pk).await.unwrap();
        // The very next step-up, through the typed client: the 412, then its retry.
        let done = in_app
            .apps()
            .call(&app, "clear", Some(json!({})), None, Some(&mut in_app_pk))
            .await;
        assert!(
            done.is_ok(),
            "signed in on node {signed_in_at}: {:?}",
            done.err()
        );
        // Asked on one node, answered on each, the same session on both.
        for asked_at in 0..n {
            for answered_at in 0..n {
                let asking = c.client(asked_at);
                asking.use_session(session.clone()).await;
                let e = asking.transport().json(clear()).await.unwrap_err();
                assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
                let d = e.details().unwrap().clone();
                let challenge =
                    calimero_identity::from_b64url(d["challenge"].as_str().unwrap()).unwrap();
                let answering = c.client(answered_at);
                answering.use_session(session.clone()).await;
                let mut retry = clear();
                if let calimero_client::Body::Json(Value::Object(b)) = &mut retry.body {
                    let _prev = b.insert(
                        "confirm".into(),
                        json!({
                            "challengeId": d["challengeId"],
                            "assertion": in_app_pk.assert(&challenge),
                        }),
                    );
                }
                let r = answering.transport().json(retry).await;
                assert!(
                    r.is_ok(),
                    "signed in on node {signed_in_at}, asked on {asked_at}, answered on \
                     {answered_at}: {:?}",
                    r.err()
                );
            }
        }
        key = in_app_pk;
    }

    // The consent page the frontend opens: asked with the app-origin session, approved
    // with the same passkey on the node's own origin.
    // Signed in on the first node, asked on the last.
    let last = n - 1;
    let mut in_app_pk = SoftAuthenticator::with_key(&key, "localhost", &app_origin(0));
    let session = c.client(0).sign_in(&mut in_app_pk).await.unwrap();
    let in_app = c.client(last);
    in_app.use_session(session).await;
    let made = post(
        &in_app,
        "consent/requests",
        json!({ "kind": "upgrade", "subject": { "app": app, "toVersion": "2.0.0" },
                "returnTo": format!("{}/1.0.0/", app_origin(0)) }),
    )
    .await
    .unwrap();
    let url = made["url"].as_str().unwrap().to_owned();
    let page = Page::new(&url);
    let mut page_key = SoftAuthenticator::with_key(&in_app_pk, "localhost", &origin(&url));
    let ((code, done), _) = page.approve(&c, &mut page_key).await;
    assert_eq!(code, 200, "{done}");
}

/// A red update of an app whose running release was uninstalled (allowed while another
/// release still ships its WASM) cannot be classified: the log no longer holds the side it
/// moves from. Under a `secondApprover` policy it still gets its first approval, recorded
/// red with the reason (`unclassified`, naming the release that is gone), and another
/// owner or admin applies it with that approval, the upgrade recorded red too.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_red_update_from_an_uninstalled_release_is_approved_first_and_applied_second() {
    const TELEMETRY: &str = "https://telemetry.example/";
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let _org = alice.orgs().create("alice org", None, None).await.unwrap();
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[43; 32]);
    for (version, outbound) in [("1.0.0", &[][..]), ("2.0.0", &[TELEMETRY][..])] {
        let d = Declares {
            outbound,
            ..Declares::default()
        };
        let (mpk, _) = release(version, &kv_wasm(), &publisher, &d);
        let _ = alice.packages().upload(mpk, None).await.unwrap();
    }
    let org = home_org(&alice).await;
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "admin" })), Some(&mut alice_pk))
        .await
        .map(wire)
        .unwrap()["token"]
        .as_str()
        .unwrap()
        .to_owned();
    let _joined = bob.orgs().invite_redeem(&token).await.unwrap();
    let notes = alice
        .apps()
        .create(typed(
            json!({ "name": "notes", "package": PACKAGE, "version": "1.0.0", "org": org }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let alice_id = account_of(&alice).await;
    let bob_id = account_of(&bob).await;
    let _set = alice
        .transport()
        .confirmed(
            Request::new(Method::POST, format!("orgs/{org}/upgrade-policy"))
                .json(json!({ "policy": { "secondApprover": true } })),
            &mut alice_pk,
        )
        .await
        .unwrap();
    // 2.0.0 ships the same WASM, so the release the app runs may go.
    let _gone = alice
        .packages()
        .version_uninstall(PACKAGE, "1.0.0")
        .await
        .unwrap();
    let _ = get_until(&bob, &format!("apps/{notes}/upgrade-policy"), |p| {
        p["policy"]["secondApprover"] == true
    })
    .await;

    // The first approval: red, the reason naming the release that is gone.
    let first = alice
        .apps()
        .approve_upgrade(&notes, "2.0.0", None, Some(&mut alice_pk))
        .await
        .unwrap();
    let approval = &first.approval;
    assert_eq!(approval.by, alice_id, "{first:?}");
    assert_eq!(
        approval.class,
        calimero_client::types::PendingApprovalClass::Red,
        "{first:?}"
    );
    assert_eq!(
        wire(&approval.reasons),
        json!([{ "code": "unclassified", "member": null, "package": PACKAGE, "version": "1.0.0" }]),
        "{first:?}"
    );
    // Alone, still the second approver's to apply.
    let e = alice
        .apps()
        .upgrade(
            &notes,
            typed(json!({ "version": "2.0.0" })),
            Some(&mut alice_pk),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("SECOND_APPROVAL_REQUIRED"), "{e}");

    // The admin applies it: the update shows the approval waiting, and the upgrade takes
    // it as the approval of this move.
    let update = get_until(&bob, &format!("apps/{notes}/update"), |u| {
        u["pendingApprovals"]
            .as_array()
            .is_some_and(|p| !p.is_empty())
    })
    .await;
    assert_eq!(
        update["pendingApprovals"][0]["by"],
        json!(alice_id),
        "{update}"
    );
    let _done = bob
        .apps()
        .upgrade(
            &notes,
            typed(json!({ "version": "2.0.0" })),
            Some(&mut bob_pk),
        )
        .await
        .unwrap();
    let moved = get_until(&alice, &format!("apps/{notes}"), |a| {
        a["release"]["version"] == "2.0.0"
    })
    .await;
    let last = &moved["lastUpgrade"];
    assert_eq!(last["class"], "red", "{moved}");
    assert_eq!(last["by"], json!(bob_id), "{moved}");
    assert_eq!(last["approvedBy"], json!(alice_id), "{moved}");
    assert_eq!(last["reasons"][0]["code"], "unclassified", "{moved}");
}
