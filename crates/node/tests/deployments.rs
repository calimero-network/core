//! Deployments: an owner's (an account's, or an organization's) list of the
//! apps it offers on a cluster, each naming one installed release. Its
//! managers change it, each with their own passkey, judged where the command
//! applies; customers create instances from its offers, which then record it;
//! the managers list the organizations with instances from it; and an app's
//! frontend opens at the release a deployment offers or an instance runs;
//! and an instance's usage is billed to the developer whose deployment it
//! came from.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

use std::time::Duration;

use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::system::Deployments;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, ops_signed, signed_up, Cluster};

const MANAGE_APPLICATION: u32 = 1 << 4;

/// A signed single-service `.mpk` shipping `frontend` files (paths under
/// `frontend/`, the first is the entry).
fn mpk(package: &str, version: &str, wasm: &[u8], frontend: &[(&str, &[u8])]) -> Vec<u8> {
    use sha2::Digest;
    let key = ed25519_dalek::SigningKey::from_bytes(&[3; 32]);
    let artifact = |path: &str, bytes: &[u8]| calimero_bundle::BundleArtifact {
        path: path.to_owned(),
        hash: hex::encode(sha2::Sha256::digest(bytes)),
        size: bytes.len() as u64,
    };
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
        wasm: Some(artifact("app.wasm", wasm)),
        abi: None,
        compute: None,
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
    let mut entries = vec![
        (
            "manifest.json".to_owned(),
            serde_json::to_vec(&manifest).unwrap(),
        ),
        ("app.wasm".to_owned(), wasm.to_vec()),
    ];
    entries.extend(frontend.iter().map(|(p, b)| ((*p).to_owned(), b.to_vec())));
    let mut tar = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, bytes) in &entries {
        let mut header = tar::Header::new_gnu();
        header.set_path(name).unwrap();
        header.set_size(bytes.len() as u64);
        header.set_cksum();
        tar.append(&header, bytes.as_slice()).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// `wasm` with a custom section named `tag`: another module that runs the
/// same, so each release ships a WASM of its own.
fn variant(wasm: &[u8], tag: &str) -> Vec<u8> {
    let mut out = wasm.to_vec();
    out.push(0);
    out.push(u8::try_from(1 + tag.len()).unwrap());
    out.push(u8::try_from(tag.len()).unwrap());
    out.extend_from_slice(tag.as_bytes());
    out
}

/// `package@version`, its WASM tagged with both.
fn release(package: &str, version: &str) -> Vec<u8> {
    mpk(
        package,
        version,
        &variant(&kv_wasm(), &format!("{package}@{version}")),
        &[],
    )
}

async fn account_of(client: &Client) -> String {
    client.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Append `cmd` to the system log as the system leader's own code would,
/// and read what `apply` answered: a command no API checked first.
async fn append(c: &Cluster, cmd: &Value) -> Result<Value, String> {
    for _ in 0..100 {
        let leader = c
            .nodes
            .iter()
            .flatten()
            .find(|r| r.node.system.status().is_leader);
        if let Some(r) = leader {
            let (_index, out) = r
                .node
                .system
                .write(serde_json::to_vec(cmd).unwrap())
                .await
                .unwrap();
            return calimero_node::system::output(&out);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("no system leader");
}

/// `pk`'s holder's consent to `method` on `subject` with `args`, as the log
/// carries it: a valid passkey assertion that skipped every API check.
fn consent_of(pk: &mut SoftAuthenticator, subject: &str, method: &str, args: &Value) -> Value {
    let statement = calimero_node::proof::Statement::new(subject, method, args);
    let challenge = statement.challenge().unwrap();
    serde_json::to_value(calimero_node::proof::Consent {
        statement,
        assertion: pk.assert(&challenge),
        at: 0,
    })
    .unwrap()
}

/// An `OfferApp` by `actor`, with their own valid consent, logged directly.
fn forged_offer(
    pk: &mut SoftAuthenticator,
    actor: &str,
    deployment: &str,
    app: &str,
    package: &str,
    version: &str,
) -> Value {
    let args = json!({ "app": app, "package": package, "version": version });
    json!({
        "op": "offerApp",
        "deployment": deployment,
        "app": app,
        "package": package,
        "version": version,
        "actor": actor,
        "at": 1_900_000_000u64,
        "consent": consent_of(pk, &format!("deployment:{deployment}"), "__offer_app", &args),
    })
}

/// A new organization of `owner`'s, named `name`.
async fn org(owner: &Client, name: &str) -> String {
    owner.post("orgs", json!({ "name": name })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// `who` joins `org` as a member, invited by `admin`.
async fn join(admin: &Client, admin_pk: &mut SoftAuthenticator, org: &str, who: &Client) {
    let token = admin
        .post_confirmed(
            &format!("orgs/{org}/invites"),
            json!({ "role": "member" }),
            admin_pk,
        )
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

async fn set_caps(
    admin: &Client,
    admin_pk: &mut SoftAuthenticator,
    org: &str,
    account: &str,
    caps: u32,
) {
    let _ok = admin
        .post_confirmed(
            &format!("orgs/{org}/capabilities"),
            json!({ "account": account, "capabilities": caps }),
            admin_pk,
        )
        .await
        .unwrap();
}

/// A new deployment of `owner` (`{"account":..}` / `{"org":..}`), created
/// by `by` with its passkey: its id.
async fn deploy(by: &Client, pk: &mut SoftAuthenticator, name: &str, owner: Value) -> String {
    by.post_confirmed("deployments", json!({ "name": name, "owner": owner }), pk)
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[allow(
    clippy::result_large_err,
    reason = "the client's own error type, as its `post` returns it"
)]
async fn offer(
    by: &Client,
    pk: &mut SoftAuthenticator,
    deployment: &str,
    app: &str,
    package: &str,
    version: &str,
) -> Result<Value, calimero_client::Error> {
    by.post_confirmed(
        &format!("deployments/{deployment}/apps"),
        json!({ "app": app, "package": package, "version": version }),
        pk,
    )
    .await
}

/// An instance of `package` from `deployment`, in `org`.
#[allow(
    clippy::result_large_err,
    reason = "the client's own error type, as its `post` returns it"
)]
async fn instance(
    by: &Client,
    deployment: &str,
    package: &str,
    version: Option<&str>,
    org: Option<&str>,
) -> Result<String, calimero_client::Error> {
    let mut body = json!({ "name": "mine", "deployment": deployment, "package": package });
    if let Some(v) = version {
        body["version"] = json!(v);
    }
    if let Some(o) = org {
        body["org"] = json!(o);
    }
    let out = by.post("apps", body).await?;
    Ok(out["appId"].as_str().unwrap().to_owned())
}

fn forbidden(e: &calimero_client::Error) {
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
}

/// An account owns a deployment it alone changes; an organization owns one
/// its `MANAGE_APPLICATION` holders change, each with their own passkey.
/// Anyone else, a member without the capability, and a manager whose
/// capability was withdrawn are refused at the API and, logged anyway with
/// their own valid consent, where the command applies. Its offers are public
/// by id; the managers' listing is theirs.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn accounts_and_organizations_own_deployments_their_managers_change() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let bob_id = account_of(&bob).await;
    let (carol, mut carol_pk) = signed_up(&c, 2, "carol").await;
    let carol_id = account_of(&carol).await;
    let _ok = alice
        .upload_package(release("com.test.notes", "1.0.0"))
        .await
        .unwrap();

    // An account's deployment: hers to change, nobody else's.
    let mine = deploy(
        &alice,
        &mut alice_pk,
        "alice-labs",
        json!({ "account": alice_id }),
    )
    .await;
    forbidden(
        &bob.post_confirmed(
            "deployments",
            json!({ "name": "not-hers", "owner": { "account": alice_id } }),
            &mut bob_pk,
        )
        .await
        .unwrap_err(),
    );
    let _ok = offer(
        &alice,
        &mut alice_pk,
        &mine,
        "notes",
        "com.test.notes",
        "1.0.0",
    )
    .await
    .unwrap();
    forbidden(
        &offer(&bob, &mut bob_pk, &mine, "notes", "com.test.notes", "1.0.0")
            .await
            .unwrap_err(),
    );
    forbidden(
        &bob.delete_confirmed(&format!("deployments/{mine}/apps/notes"), &mut bob_pk)
            .await
            .unwrap_err(),
    );
    // Logged anyway, with bob's own consent: refused where it applies.
    let e = append(
        &c,
        &forged_offer(
            &mut bob_pk,
            &bob_id,
            &mine,
            "evil",
            "com.test.notes",
            "1.0.0",
        ),
    )
    .await
    .unwrap_err();
    assert!(e.contains("manage"), "{e}");
    // No consent at all: refused before its own rules are asked.
    let mut bare = forged_offer(
        &mut alice_pk,
        &alice_id,
        &mine,
        "x",
        "com.test.notes",
        "1.0.0",
    );
    bare["consent"] = Value::Null;
    assert!(append(&c, &bare).await.is_err());

    // Public by id: the name, the owner's kind and name, the offers.
    let anon = c.client(1);
    let public = anon.get(&format!("deployments/{mine}")).await.unwrap();
    assert_eq!(public["name"], "alice-labs");
    assert_eq!(
        public["owner"],
        json!({ "kind": "account", "name": "alice" })
    );
    assert_eq!(
        public["apps"],
        json!([{ "app": "notes", "package": "com.test.notes", "version": "1.0.0" }])
    );
    // The managers' listing.
    let listed = alice.get("deployments").await.unwrap();
    assert_eq!(listed["deployments"][0]["id"], json!(mine), "{listed}");
    let listed = bob.get("deployments").await.unwrap();
    assert_eq!(listed["deployments"], json!([]), "{listed}");

    // Withdrawn, then deleted once it offers nothing.
    let e = alice
        .delete_confirmed(&format!("deployments/{mine}"), &mut alice_pk)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("withdraw"), "{e}");
    let _ok = alice
        .delete_confirmed(&format!("deployments/{mine}/apps/notes"), &mut alice_pk)
        .await
        .unwrap();
    let _ok = alice
        .delete_confirmed(&format!("deployments/{mine}"), &mut alice_pk)
        .await
        .unwrap();
    let e = anon.get(&format!("deployments/{mine}")).await.unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");

    // An organization's deployment: alice owns the org, carol holds
    // MANAGE_APPLICATION org-wide, bob is a plain member.
    let acme = org(&alice, "Acme").await;
    join(&alice, &mut alice_pk, &acme, &bob).await;
    join(&alice, &mut alice_pk, &acme, &carol).await;
    set_caps(&alice, &mut alice_pk, &acme, &carol_id, MANAGE_APPLICATION).await;
    // carol needs to see the release to offer it: she shares the org with
    // its installer.
    forbidden(
        &bob.post_confirmed(
            "deployments",
            json!({ "name": "acme", "owner": { "org": acme } }),
            &mut bob_pk,
        )
        .await
        .unwrap_err(),
    );
    let theirs = deploy(&carol, &mut carol_pk, "acme", json!({ "org": acme })).await;
    let _ok = offer(
        &carol,
        &mut carol_pk,
        &theirs,
        "notes",
        "com.test.notes",
        "1.0.0",
    )
    .await
    .unwrap();
    // The org's owner manages it too.
    let listed = alice.get("deployments").await.unwrap();
    assert_eq!(listed["deployments"][0]["id"], json!(theirs), "{listed}");
    assert_eq!(
        anon.get(&format!("deployments/{theirs}")).await.unwrap()["owner"],
        json!({ "kind": "org", "name": "Acme" })
    );
    forbidden(
        &offer(
            &bob,
            &mut bob_pk,
            &theirs,
            "more",
            "com.test.notes",
            "1.0.0",
        )
        .await
        .unwrap_err(),
    );
    let e = append(
        &c,
        &forged_offer(
            &mut bob_pk,
            &bob_id,
            &theirs,
            "more",
            "com.test.notes",
            "1.0.0",
        ),
    )
    .await
    .unwrap_err();
    assert!(e.contains("manage"), "a member without the capability: {e}");

    // Carol's capability withdrawn: her next change is refused, at the API
    // and where it applies.
    set_caps(&alice, &mut alice_pk, &acme, &carol_id, 0).await;
    forbidden(
        &carol
            .delete_confirmed(&format!("deployments/{theirs}/apps/notes"), &mut carol_pk)
            .await
            .unwrap_err(),
    );
    let e = append(
        &c,
        &forged_offer(
            &mut carol_pk,
            &carol_id,
            &theirs,
            "more",
            "com.test.notes",
            "1.0.0",
        ),
    )
    .await
    .unwrap_err();
    assert!(e.contains("manage"), "a withdrawn capability: {e}");
    // Granted again, then removed from the organization: refused the same.
    set_caps(&alice, &mut alice_pk, &acme, &carol_id, MANAGE_APPLICATION).await;
    let _ok = alice
        .delete_confirmed(&format!("orgs/{acme}/members/{carol_id}"), &mut alice_pk)
        .await
        .unwrap();
    forbidden(
        &offer(
            &carol,
            &mut carol_pk,
            &theirs,
            "more",
            "com.test.notes",
            "1.0.0",
        )
        .await
        .unwrap_err(),
    );
    let e = append(
        &c,
        &forged_offer(
            &mut carol_pk,
            &carol_id,
            &theirs,
            "more",
            "com.test.notes",
            "1.0.0",
        ),
    )
    .await
    .unwrap_err();
    assert!(e.contains("manage"), "a removed member: {e}");
    let _ok = alice
        .delete_confirmed(&format!("deployments/{theirs}/apps/notes"), &mut alice_pk)
        .await
        .unwrap();
}

/// Two deployments offering the same release are independent: each lists
/// only the organizations with instances from it, a withdraw waits for its
/// own instances only, and the release cannot be uninstalled while either
/// offers it. An instance is created from a deployment only at a release it
/// offers, by a customer who need not see that release otherwise.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn deployments_of_one_release_are_independent() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let (dave, mut dave_pk) = signed_up(&c, 1, "dave").await;
    let dave_id = account_of(&dave).await;
    let (carol, _carol_pk) = signed_up(&c, 2, "carol").await;
    let (erin, _erin_pk) = signed_up(&c, 0, "erin").await;
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;

    let notes = release("com.test.notes", "1.0.0");
    let _ok = alice.upload_package(notes.clone()).await.unwrap();
    // Dave installs the same release: it is his to offer too.
    let _ok = dave.upload_package(notes).await.unwrap();
    let _ok = alice
        .upload_package(release("com.test.notes", "2.0.0"))
        .await
        .unwrap();
    let _ok = alice
        .upload_package(release("com.test.board", "1.0.0"))
        .await
        .unwrap();

    let da = deploy(
        &alice,
        &mut alice_pk,
        "alpha",
        json!({ "account": alice_id }),
    )
    .await;
    let db = deploy(&dave, &mut dave_pk, "beta", json!({ "account": dave_id })).await;
    for (who, pk, d) in [(&alice, &mut alice_pk, &da), (&dave, &mut dave_pk, &db)] {
        let _ok = offer(who, pk, d, "notes", "com.test.notes", "1.0.0")
            .await
            .unwrap();
    }

    // Customers: carol's org from alpha, erin's from beta. Neither installed
    // or sees the release.
    let globex = org(&carol, "Globex").await;
    let initech = org(&erin, "Initech").await;
    let from_a = instance(&carol, &da, "com.test.notes", None, Some(&globex))
        .await
        .unwrap();
    let _from_b = instance(&erin, &db, "com.test.notes", Some("1.0.0"), Some(&initech))
        .await
        .unwrap();
    let rec = carol.get(&format!("apps/{from_a}")).await.unwrap();
    assert_eq!(rec["deployment"], json!(da), "{rec}");

    // Only the offered release, of an offered package.
    for (package, version) in [
        ("com.test.notes", Some("2.0.0")),
        ("com.test.board", None),
        ("com.test.board", Some("1.0.0")),
    ] {
        let e = instance(&carol, &da, package, version, Some(&globex))
            .await
            .unwrap_err();
        assert!(
            e.to_string().contains("offer"),
            "{package} {version:?}: {e}"
        );
    }
    let e = instance(&carol, "nope", "com.test.notes", None, Some(&globex))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");

    // Each deployment lists its own customers only, for its managers only.
    let listed = alice.get(&format!("deployments/{da}/orgs")).await.unwrap();
    let orgs = listed["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 1, "{listed}");
    assert_eq!(orgs[0]["orgId"], json!(globex));
    assert_eq!(orgs[0]["name"], "Globex");
    assert_eq!(orgs[0]["instances"], 1);
    assert_eq!(
        orgs[0]["apps"],
        json!([{ "app": "notes", "version": "1.0.0" }])
    );
    assert!(orgs[0].get("members").is_none(), "{listed}");
    let listed = dave.get(&format!("deployments/{db}/orgs")).await.unwrap();
    let orgs = listed["orgs"].as_array().unwrap();
    assert_eq!(orgs.len(), 1, "{listed}");
    assert_eq!(orgs[0]["orgId"], json!(initech));
    forbidden(
        &bob.get(&format!("deployments/{da}/orgs"))
            .await
            .unwrap_err(),
    );
    forbidden(
        &dave
            .get(&format!("deployments/{da}/orgs"))
            .await
            .unwrap_err(),
    );

    // A withdraw waits for the deployment's own instances.
    let e = alice
        .delete_confirmed(&format!("deployments/{da}/apps/notes"), &mut alice_pk)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("instance"), "{e}");
    let _ok = carol.delete(&format!("apps/{from_a}")).await.unwrap();
    let _ok = alice
        .delete_confirmed(&format!("deployments/{da}/apps/notes"), &mut alice_pk)
        .await
        .unwrap();
    // Beta still offers it, and its instance runs on.
    let beta = alice.get(&format!("deployments/{db}")).await.unwrap();
    assert_eq!(beta["apps"][0]["version"], "1.0.0", "{beta}");
    let e = dave
        .delete_confirmed(&format!("deployments/{db}/apps/notes"), &mut dave_pk)
        .await
        .unwrap_err();
    assert!(e.to_string().contains("instance"), "{e}");

    // Offered by beta: its first installer cannot uninstall it.
    let e = alice
        .delete("packages/com.test.notes/1.0.0")
        .await
        .unwrap_err();
    assert!(e.to_string().contains("offers"), "{e}");
    // A release nobody offers keeps today's rules.
    let _ok = alice.delete("packages/com.test.notes/2.0.0").await.unwrap();
}

/// An app's frontend opens at the release its deployment offers
/// (`/d/{deployment}/`, which also tells the page the deployment) or its
/// instance runs (`/app/{app}/`), whatever newer release `/` points at; the
/// well-known file lists the deployments offering the package.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn frontends_open_the_deployed_release() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let (carol, _carol_pk) = signed_up(&c, 1, "carol").await;
    let site = |version: &str, html: &'static [u8]| {
        mpk(
            "com.test.site",
            version,
            &variant(&kv_wasm(), version),
            &[("frontend/index.html", html)],
        )
    };
    let _ok = alice
        .upload_package(site("1.0.0", b"<p>one"))
        .await
        .unwrap();
    let _ok = alice
        .upload_package(release("com.test.other", "1.0.0"))
        .await
        .unwrap();
    let d = deploy(
        &alice,
        &mut alice_pk,
        "alpha",
        json!({ "account": alice_id }),
    )
    .await;
    let _ok = offer(&alice, &mut alice_pk, &d, "site", "com.test.site", "1.0.0")
        .await
        .unwrap();
    let app = instance(&carol, &d, "com.test.site", None, None)
        .await
        .unwrap();
    let other = alice
        .post("apps", json!({ "name": "o", "package": "com.test.other" }))
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    // A newer release is installed: `/` moves to it.
    let _ok = alice
        .upload_package(site("1.1.0", b"<p>two"))
        .await
        .unwrap();

    let label = calimero_node::app_host::label("com.test.site");
    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let host = format!("{label}.localhost");
    let origin = format!("http://{host}:{port}");
    let web = reqwest::Client::builder()
        .resolve(&host, ([127, 0, 0, 1], port).into())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let get = |path: String| {
        web.get(format!("{origin}{path}"))
            .header("accept", "text/html")
            .send()
    };
    let location = |r: &reqwest::Response| {
        assert_eq!(r.status(), 302, "{r:?}");
        r.headers()["location"].to_str().unwrap().to_owned()
    };

    assert_eq!(location(&get("/".into()).await.unwrap()), "/1.1.0/");
    assert_eq!(
        location(&get(format!("/app/{app}/")).await.unwrap()),
        "/1.0.0/"
    );
    assert_eq!(
        location(&get(format!("/d/{d}/")).await.unwrap()),
        format!("/1.0.0/?deployment={d}")
    );
    let page = get("/1.0.0/".into()).await.unwrap();
    assert_eq!(page.bytes().await.unwrap().as_ref(), b"<p>one");
    let known: Value = get("/.well-known/calimero.json".into())
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        known["deployments"],
        json!([{ "id": d, "name": "alpha", "app": "site", "version": "1.0.0" }])
    );
    // Not another package's instance, nor an unknown one; files only.
    assert_eq!(get(format!("/app/{other}/")).await.unwrap().status(), 404);
    assert_eq!(get("/app/nope/".into()).await.unwrap().status(), 404);
    assert_eq!(get("/d/nope/".into()).await.unwrap().status(), 404);
    let post = web.post(format!("{origin}/d/{d}/")).send().await.unwrap();
    assert_eq!(post.status(), 404);

    // The deployment moves to the newer release: `/d/` follows it, the
    // instance's link stays on the release it runs.
    let _ok = offer(&alice, &mut alice_pk, &d, "site", "com.test.site", "1.1.0")
        .await
        .unwrap();
    // Served from this node's state, which applies the change a moment
    // after the leader committed it.
    let moved = format!("/1.1.0/?deployment={d}");
    let mut seen = String::new();
    for _ in 0..50 {
        seen = location(&get(format!("/d/{d}/")).await.unwrap());
        if seen == moved {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(seen, moved);
    assert_eq!(
        location(&get(format!("/app/{app}/")).await.unwrap()),
        "/1.0.0/"
    );
}

/// Every deployment has a frontend host of its own,
/// `{deployment_label}.{apps-domain}` (what the resolver in front routes to
/// the deployment's cluster): it serves what `/d/{deployment}/` serves on the
/// package's host, names its deployment first in the well-known file, and a
/// passkey ceremony from it opens a session bound to the package. Two
/// deployments of one package get two hosts; an unknown one, or one
/// offering apps of several packages, answers 404; the package's own host
/// is unchanged.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn each_deployment_serves_its_frontend_on_its_own_host() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let site = |version: &str, html: &'static [u8]| {
        mpk(
            "com.test.site",
            version,
            &variant(&kv_wasm(), version),
            &[("frontend/index.html", html)],
        )
    };
    let _ok = alice
        .upload_package(site("1.0.0", b"<p>one"))
        .await
        .unwrap();
    let _ok = alice
        .upload_package(site("1.1.0", b"<p>two"))
        .await
        .unwrap();
    let _ok = alice
        .upload_package(release("com.test.other", "1.0.0"))
        .await
        .unwrap();
    let created = alice
        .post_confirmed(
            "deployments",
            json!({ "name": "alpha", "owner": { "account": alice_id } }),
            &mut alice_pk,
        )
        .await
        .unwrap();
    let alpha = created["id"].as_str().unwrap().to_owned();
    let alpha_label = calimero_node::app_host::deployment_label(&alpha);
    let alpha_host = format!("{alpha_label}.localhost");
    assert_eq!(created["host"], json!(alpha_host), "{created}");
    let beta = deploy(
        &alice,
        &mut alice_pk,
        "beta",
        json!({ "account": alice_id }),
    )
    .await;
    let mixed = deploy(
        &alice,
        &mut alice_pk,
        "mixed",
        json!({ "account": alice_id }),
    )
    .await;
    let _ok = offer(
        &alice,
        &mut alice_pk,
        &alpha,
        "site",
        "com.test.site",
        "1.0.0",
    )
    .await
    .unwrap();
    let _ok = offer(
        &alice,
        &mut alice_pk,
        &beta,
        "site",
        "com.test.site",
        "1.1.0",
    )
    .await
    .unwrap();
    for (app, package) in [("site", "com.test.site"), ("other", "com.test.other")] {
        let _ok = offer(&alice, &mut alice_pk, &mixed, app, package, "1.0.0")
            .await
            .unwrap();
    }
    // Read back by anyone: the host travels with the id.
    let read = c
        .client(2)
        .get(&format!("deployments/{beta}"))
        .await
        .unwrap();
    let beta_host = format!(
        "{}.localhost",
        calimero_node::app_host::deployment_label(&beta)
    );
    assert_eq!(read["host"], json!(beta_host), "{read}");
    assert_ne!(alpha_host, beta_host, "two deployments, two hosts");

    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let browser = |host: &str| {
        reqwest::Client::builder()
            .resolve(host, ([127, 0, 0, 1], port).into())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
    };
    let get = |host: &str, path: &str| {
        browser(host)
            .get(format!("http://{host}:{port}{path}"))
            .header("accept", "text/html")
            .send()
    };
    let location = |r: &reqwest::Response| {
        assert_eq!(r.status(), 302, "{r:?}");
        r.headers()["location"].to_str().unwrap().to_owned()
    };

    // Each host opens its deployment's release, telling the page which.
    assert_eq!(
        location(&get(&alpha_host, "/").await.unwrap()),
        format!("/1.0.0/?deployment={alpha}")
    );
    assert_eq!(
        location(&get(&beta_host, "/").await.unwrap()),
        format!("/1.1.0/?deployment={beta}")
    );
    let page = get(&alpha_host, "/1.0.0/").await.unwrap();
    assert_eq!(page.status(), 200);
    assert!(page.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("default-src 'self'"));
    assert_eq!(page.bytes().await.unwrap().as_ref(), b"<p>one");
    assert_eq!(
        location(&get(&alpha_host, &format!("/d/{alpha}/x")).await.unwrap()),
        format!("/1.0.0/x?deployment={alpha}")
    );
    // Another deployment's link is not this host's; nor are node routes.
    assert_eq!(
        get(&alpha_host, &format!("/d/{beta}/"))
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(get(&alpha_host, "/health").await.unwrap().status(), 404);
    let known: Value = get(&beta_host, "/.well-known/calimero.json")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(known["package"], json!("com.test.site"));
    assert_eq!(known["deployment"], json!(beta));
    assert_eq!(known["deployments"][0]["id"], json!(beta), "{known}");
    assert_eq!(known["deployments"].as_array().unwrap().len(), 3, "{known}");
    // Unknown, or no one package: not served.
    let stranger = format!(
        "{}.localhost",
        calimero_node::app_host::deployment_label("ffffffffffffffffffffffffffffffff")
    );
    assert_eq!(get(&stranger, "/").await.unwrap().status(), 404);
    assert_eq!(get(&stranger, "/health").await.unwrap().status(), 404);
    let mixed_host = format!(
        "{}.localhost",
        calimero_node::app_host::deployment_label(&mixed)
    );
    assert_eq!(get(&mixed_host, "/").await.unwrap().status(), 404);
    // The package's own host and its `/d/` link are unchanged.
    let package_host = format!(
        "{}.localhost",
        calimero_node::app_host::label("com.test.site")
    );
    assert_eq!(location(&get(&package_host, "/").await.unwrap()), "/1.1.0/");
    assert_eq!(
        location(&get(&package_host, &format!("/d/{alpha}/")).await.unwrap()),
        format!("/1.0.0/?deployment={alpha}")
    );
    let known: Value = get(&package_host, "/.well-known/calimero.json")
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(known["deployment"], Value::Null);

    // Signing up on a deployment's host binds the session to its package.
    let origin = format!("http://{alpha_host}:{port}");
    let dave = c.client(1);
    let mut dave_pk = SoftAuthenticator::new("localhost", &origin);
    let _s = dave.sign_up(&mut dave_pk, "dave").await.unwrap();
    let mine = instance(&dave, &alpha, "com.test.site", None, None)
        .await
        .unwrap();
    let _ok = dave
        .call(&mine, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();
    assert!(dave
        .post("apps", json!({ "name": "x", "package": "com.test.other" }))
        .await
        .is_err());
    assert!(dave
        .create_api_key(json!({ "label": "k" }), &mut dave_pk)
        .await
        .is_err());
    // And signing in there again, too.
    let dave_again = c.client(2);
    let mut again_pk = SoftAuthenticator::with_key(&dave_pk, "localhost", &origin);
    let _s = dave_again.sign_in(&mut again_pk).await.unwrap();
    assert!(dave_again
        .create_api_key(json!({ "label": "k" }), &mut again_pk)
        .await
        .is_err());
    let _ok = dave_again
        .call(&mine, "set", json!({ "key": "k", "value": "w" }), None)
        .await
        .unwrap();
    // CORS admits the deployment's host as an origin.
    let pre = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, format!("{}/health", c.urls[0]))
        .header("origin", &origin)
        .header("access-control-request-method", "POST")
        .send()
        .await
        .unwrap();
    assert_eq!(
        pre.headers()
            .get("access-control-allow-origin")
            .map(|v| v.to_str().unwrap().to_owned()),
        Some(origin.clone())
    );
}

/// Every redirect a frontend host answers keeps the request's query: an
/// invite link (`/?invite=...`) still carries its token once it lands on
/// the release, after the `deployment` a `/d/` link adds. An empty query adds
/// nothing; one past the bound is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn frontend_redirects_keep_the_query() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let _ok = alice
        .upload_package(mpk(
            "com.test.site",
            "1.0.0",
            &variant(&kv_wasm(), "1.0.0"),
            &[("frontend/index.html", b"<p>one")],
        ))
        .await
        .unwrap();
    let d = deploy(
        &alice,
        &mut alice_pk,
        "alpha",
        json!({ "account": alice_id }),
    )
    .await;
    let _ok = offer(&alice, &mut alice_pk, &d, "site", "com.test.site", "1.0.0")
        .await
        .unwrap();
    let app = instance(&alice, &d, "com.test.site", None, None)
        .await
        .unwrap();

    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let package_host = format!(
        "{}.localhost",
        calimero_node::app_host::label("com.test.site")
    );
    let deployment_host = format!(
        "{}.localhost",
        calimero_node::app_host::deployment_label(&d)
    );
    let location = |host: String, path: String| async move {
        let r = reqwest::Client::builder()
            .resolve(&host, ([127, 0, 0, 1], port).into())
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap()
            .get(format!("http://{host}:{port}{path}"))
            .header("accept", "text/html")
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 302, "{path}: {r:?}");
        r.headers()["location"].to_str().unwrap().to_owned()
    };
    let cases = [
        (
            &package_host,
            "/?invite=t0k".to_owned(),
            "/1.0.0/?invite=t0k".to_owned(),
        ),
        (&package_host, "/".to_owned(), "/1.0.0/".to_owned()),
        (&package_host, "/?".to_owned(), "/1.0.0/".to_owned()),
        (
            &package_host,
            format!("/app/{app}/join?invite=t0k&x=1"),
            "/1.0.0/join?invite=t0k&x=1".to_owned(),
        ),
        (
            &package_host,
            format!("/d/{d}/?invite=t0k"),
            format!("/1.0.0/?deployment={d}&invite=t0k"),
        ),
        (
            &deployment_host,
            "/?invite=t0k".to_owned(),
            format!("/1.0.0/?deployment={d}&invite=t0k"),
        ),
        (
            &deployment_host,
            format!("/app/{app}/?invite=t0k"),
            "/1.0.0/?invite=t0k".to_owned(),
        ),
    ];
    for (host, path, want) in cases {
        assert_eq!(location(host.clone(), path.clone()).await, want, "{path}");
    }
    // A query past the bound is refused, not carried on.
    let long = reqwest::Client::builder()
        .resolve(&package_host, ([127, 0, 0, 1], port).into())
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
        .get(format!(
            "http://{package_host}:{port}/?invite={}",
            "a".repeat(5000)
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(long.status(), 414);
}

/// `/app/{app}/` opens the release that instance runs, never simply the newest
/// one shipping its WASM: releases may share a WASM and differ in their
/// frontend, which works with its own release only. Created at a release, or
/// from a deployment's offer, it opens that one; once upgraded (to a release
/// with the same WASM too), the new one.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_instance_link_opens_the_release_that_instance_runs() {
    let c = Cluster::start_with(3, |cfg| cfg.apps_domain = Some("localhost".into())).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    // One WASM in three releases; by string "1.0.0" < "10.0.0" < "9.0.0".
    let wasm = variant(&kv_wasm(), "shared");
    for version in ["1.0.0", "9.0.0", "10.0.0"] {
        let page = format!("<p>{version}");
        let _ok = alice
            .upload_package(mpk(
                "com.test.site",
                version,
                &wasm,
                &[("frontend/index.html", page.as_bytes())],
            ))
            .await
            .unwrap();
    }
    let app = alice
        .post(
            "apps",
            json!({ "name": "s", "package": "com.test.site", "version": "1.0.0" }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let d = deploy(
        &alice,
        &mut alice_pk,
        "nine",
        json!({ "account": alice_id }),
    )
    .await;
    let _ok = offer(&alice, &mut alice_pk, &d, "site", "com.test.site", "9.0.0")
        .await
        .unwrap();
    let offered = instance(&alice, &d, "com.test.site", None, None)
        .await
        .unwrap();

    let port = url::Url::parse(&c.urls[0]).unwrap().port().unwrap();
    let host = format!(
        "{}.localhost",
        calimero_node::app_host::label("com.test.site")
    );
    let location = |app: String| {
        let host = host.clone();
        async move {
            let r = reqwest::Client::builder()
                .resolve(&host, ([127, 0, 0, 1], port).into())
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap()
                .get(format!("http://{host}:{port}/app/{app}/"))
                .header("accept", "text/html")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 302, "{r:?}");
            r.headers()["location"].to_str().unwrap().to_owned()
        }
    };
    assert_eq!(location(app.clone()).await, "/1.0.0/");
    assert_eq!(location(offered.clone()).await, "/9.0.0/");

    // Upgraded to 10.0.0 (the same WASM): the link follows it.
    let _ok = alice
        .upgrade_app(&app, json!({ "version": "10.0.0" }), &mut alice_pk)
        .await
        .unwrap();
    assert_eq!(location(app).await, "/10.0.0/");
    assert_eq!(location(offered).await, "/9.0.0/");
}

/// Where deployments are granted, creating a deployment and offering an app
/// need a live deployer grant (refused at the API and where it applies); a
/// customer without one still creates instances from the offers.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn on_a_granted_cluster_deploying_needs_a_grant() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance.deployments = Deployments::Granted;
    })
    .await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;
    let not_granted = |e: &calimero_client::Error| {
        assert_eq!(e.code(), Some("DEPLOY_NOT_GRANTED"), "{e}");
    };

    not_granted(
        &alice
            .post_confirmed(
                "deployments",
                json!({ "name": "alpha", "owner": { "account": alice_id } }),
                &mut alice_pk,
            )
            .await
            .unwrap_err(),
    );
    let _ok = ops_signed(
        &ops,
        "ops/deployers",
        json!({ "deployer": { "account": alice_id } }),
    )
    .await
    .unwrap();
    let _ok = alice
        .upload_package(release("com.test.notes", "1.0.0"))
        .await
        .unwrap();
    let _ok = alice
        .upload_package(release("com.test.notes", "1.1.0"))
        .await
        .unwrap();
    let d = deploy(
        &alice,
        &mut alice_pk,
        "alpha",
        json!({ "account": alice_id }),
    )
    .await;
    let _ok = offer(
        &alice,
        &mut alice_pk,
        &d,
        "notes",
        "com.test.notes",
        "1.0.0",
    )
    .await
    .unwrap();
    // A customer without a grant: from the offer, yes.
    let app = instance(&bob, &d, "com.test.notes", None, None)
        .await
        .unwrap();
    let _ok = bob
        .call(&app, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();

    // Revoked: no new offer, at the API or where it applies; the instance
    // runs on.
    let _ok = ops_signed(
        &ops,
        &format!("ops/deployers/account:{alice_id}/revoke"),
        json!({}),
    )
    .await
    .unwrap();
    not_granted(
        &offer(
            &alice,
            &mut alice_pk,
            &d,
            "notes",
            "com.test.notes",
            "1.1.0",
        )
        .await
        .unwrap_err(),
    );
    let e = append(
        &c,
        &forged_offer(
            &mut alice_pk,
            &alice_id,
            &d,
            "notes",
            "com.test.notes",
            "1.1.0",
        ),
    )
    .await
    .unwrap_err();
    assert!(e.contains("deployer grant"), "{e}");
    let _ok = bob
        .call(&app, "set", json!({ "key": "k", "value": "w" }), None)
        .await
        .unwrap();
}

/// The value of `metric{labels}` in Prometheus text, if the series is there.
fn sample(text: &str, metric: &str, labels: &str) -> Option<f64> {
    let head = format!("{metric}{{{labels}}} ");
    text.lines()
        .find_map(|l| l.strip_prefix(&head))
        .map(|v| v.parse().unwrap())
}

/// Two developers, an account and an organization, each offer an app on one
/// shared cluster; one customer organization makes an instance of each. Every
/// `calimero_app_*` series of an instance names its customer's org as `org`
/// (quotas stay the customer's) and the deployment's owner as `developer`, so
/// a call on developer A's instance is counted against A and never against B.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_instances_usage_is_billed_to_its_deployments_developer() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.metrics_listen = Some("127.0.0.1:0".parse().unwrap());
    })
    .await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let (dave, mut dave_pk) = signed_up(&c, 1, "dave").await;
    let (carol, _carol_pk) = signed_up(&c, 2, "carol").await;

    // Developer A is alice's account; developer B is dave's organization.
    let _ok = alice
        .upload_package(release("com.test.notes", "1.0.0"))
        .await
        .unwrap();
    let _ok = dave
        .upload_package(release("com.test.board", "1.0.0"))
        .await
        .unwrap();
    let dev_b = org(&dave, "Bee Labs").await;
    let da = deploy(
        &alice,
        &mut alice_pk,
        "alpha",
        json!({ "account": alice_id }),
    )
    .await;
    let db = deploy(&dave, &mut dave_pk, "beta", json!({ "org": dev_b })).await;
    let _ok = offer(
        &alice,
        &mut alice_pk,
        &da,
        "notes",
        "com.test.notes",
        "1.0.0",
    )
    .await
    .unwrap();
    let _ok = offer(&dave, &mut dave_pk, &db, "board", "com.test.board", "1.0.0")
        .await
        .unwrap();

    // The customer: one organization with an instance from each.
    let globex = org(&carol, "Globex").await;
    let of_a = instance(&carol, &da, "com.test.notes", None, Some(&globex))
        .await
        .unwrap();
    let of_b = instance(&carol, &db, "com.test.board", None, Some(&globex))
        .await
        .unwrap();
    for i in 0..3 {
        let _ = carol
            .call(
                &of_a,
                "set",
                json!({ "key": format!("k{i}"), "value": "v" }),
                None,
            )
            .await
            .unwrap();
    }
    let _ = carol
        .call(&of_b, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap();

    for r in c.nodes.iter().flatten() {
        r.node.sample_usage();
    }
    let http = reqwest::Client::new();
    let mut texts = Vec::new();
    for r in c.nodes.iter().flatten() {
        let addr = r.metrics_addr.unwrap();
        let text = http
            .get(format!("http://{addr}/metrics"))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        texts.push(text);
    }
    let summed = |metric: &str, labels: &str| -> f64 {
        texts.iter().filter_map(|t| sample(t, metric, labels)).sum()
    };
    let a = format!("app=\"{of_a}\",org=\"{globex}\",developer=\"account:{alice_id}\"");
    let b = format!("app=\"{of_b}\",org=\"{globex}\",developer=\"{dev_b}\"");
    let mutations = |labels: &str| {
        summed(
            "calimero_app_calls_total",
            &format!("{labels},kind=\"mutation\""),
        )
    };
    // `init` and the calls: 4 on A's instance, 2 on B's, each counted once.
    assert_eq!(mutations(&a), 4.0, "{texts:#?}");
    assert_eq!(mutations(&b), 2.0, "{texts:#?}");
    assert!(summed("calimero_app_wasm_cpu_seconds_total", &a) > 0.0);
    assert!(summed("calimero_app_wasm_cpu_seconds_total", &b) > 0.0);
    for t in &texts {
        assert!(sample(t, "calimero_app_state_bytes", &a).is_some(), "{t}");
        for line in t.lines().filter(|l| l.starts_with("calimero_app_")) {
            if line.contains(&format!("app=\"{of_a}\"")) {
                assert!(line.contains(&a), "A's app billed elsewhere: {line}");
            }
            if line.contains(&format!("app=\"{of_b}\"")) {
                assert!(line.contains(&b), "B's app billed elsewhere: {line}");
            }
        }
        // The customer is never the developer of either.
        assert!(!t.contains(&format!("developer=\"{globex}\"")), "{t}");
    }
}

/// D78 (Xabi, 11-09): the daily fuel cap follows the developer, the identity an
/// instance's usage is billed to (`usage::developer`). Two customers' instances of
/// one developer's deployment are two payers, but one developer: their fuel adds up
/// against one cap. An instance whose own payer has burned almost nothing is refused
/// once the developer's total is at the cap; the developer reads that total, and a
/// customer, who is the developer of nothing, reads none.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn two_payers_of_one_developers_apps_share_its_fuel_cap() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let (carol, _carol_pk) = signed_up(&c, 1, "carol").await;
    let (erin, _erin_pk) = signed_up(&c, 2, "erin").await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;

    let _ok = alice
        .upload_package(release("com.test.notes", "1.0.0"))
        .await
        .unwrap();
    let alpha = deploy(
        &alice,
        &mut alice_pk,
        "alpha",
        json!({ "account": alice_id }),
    )
    .await;
    let _ok = offer(
        &alice,
        &mut alice_pk,
        &alpha,
        "notes",
        "com.test.notes",
        "1.0.0",
    )
    .await
    .unwrap();
    let globex = org(&carol, "Globex").await;
    let initech = org(&erin, "Initech").await;
    let at_globex = instance(&carol, &alpha, "com.test.notes", None, Some(&globex))
        .await
        .unwrap();
    let at_initech = instance(&erin, &alpha, "com.test.notes", None, Some(&initech))
        .await
        .unwrap();

    // Globex's instance burns; Initech's has only run its `init`.
    for i in 0..10 {
        let _r = carol
            .call(
                &at_globex,
                "set",
                json!({ "key": format!("k{i}"), "value": "v" }),
                None,
            )
            .await
            .unwrap();
    }
    let fuel = |v: &Value, k: &str| -> u64 {
        v[k].as_str()
            .unwrap_or_else(|| panic!("{k} is not a decimal string: {v}"))
            .parse()
            .unwrap()
    };
    let today = |v: Value| fuel(&v["today"], "fuel");
    let globex_fuel = today(carol.get(&format!("apps/{at_globex}/usage")).await.unwrap());
    let initech_fuel = today(erin.get(&format!("apps/{at_initech}/usage")).await.unwrap());
    assert!(
        initech_fuel < globex_fuel,
        "{initech_fuel} vs {globex_fuel}"
    );
    // The developer's total is both instances'.
    let developer = fuel(&alice.usage().await.unwrap(), "developerFuelToday");
    assert_eq!(developer, globex_fuel + initech_fuel);
    // A customer is the developer of neither.
    assert_eq!(
        fuel(
            &erin.get(&format!("orgs/{initech}/usage")).await.unwrap(),
            "developerFuelToday"
        ),
        0
    );

    // A cap above what either payer burned, at what the developer did.
    let _ok = ops_signed(
        &ops,
        "ops/quotas",
        json!({ "maxFuelPerDeveloperPerDay": developer.to_string() }),
    )
    .await
    .unwrap();
    for r in c.nodes.iter().flatten() {
        for _ in 0..100 {
            if r.node.sys(|s| s.quotas.max_fuel_per_developer_per_day) == developer {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
    let err = erin
        .call(
            &at_initech,
            "set",
            json!({ "key": "k", "value": "v" }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
    let err = carol
        .call(&at_globex, "set", json!({ "key": "k", "value": "v" }), None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
}
