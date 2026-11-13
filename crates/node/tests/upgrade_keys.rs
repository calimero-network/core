//! Upgrade keys: an API key the owner makes with a passkey to upgrade one
//! app to signed releases of its package, so CI deploys without a passkey on
//! each upgrade. The key upgrades that app and does nothing else; revoking it
//! stops it at once; a raw bundle, another package or another signer is
//! refused; and a node that saw only the log cannot forge its proof.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

use std::time::Duration;

use calimero_client::Client;
use calimero_node::system::SystemCmd;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, typed, wire, Cluster};

/// A signed single-service `.mpk`, the shape `meroctl app pack` signs.
fn mpk(package: &str, version: &str, wasm: &[u8], key: &ed25519_dalek::SigningKey) -> Vec<u8> {
    use sha2::Digest;
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
        compute: None,
        abi: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        upgrades: None,
        outbound: Vec::new(),
        services: None,
        pipelines: Vec::new(),
        signature: None,
    };
    let mut manifest = serde_json::to_value(&manifest).unwrap();
    calimero_bundle::sign_manifest_json(&mut manifest, key).unwrap();
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
    tar.into_inner().unwrap().finish().unwrap()
}

/// `wasm` with a custom section named `tag`: another module that runs the same.
fn variant(wasm: &[u8], tag: &str) -> Vec<u8> {
    let mut out = wasm.to_vec();
    let body_len = 1 + tag.len();
    out.push(0);
    out.push(u8::try_from(body_len).unwrap());
    out.push(u8::try_from(tag.len()).unwrap());
    out.extend_from_slice(tag.as_bytes());
    out
}

/// Append `cmd` to the system log as the system leader's own code would,
/// and read what `apply` answered.
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

async fn account_id(cl: &Client) -> String {
    cl.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// The WASM hash `package@version` ships, from the installer's listing.
async fn bundle_of(owner: &Client, package: &str, version: &str) -> String {
    let p = owner.packages().get(package).await.map(wire).unwrap();
    let versions = p["versions"].as_array().cloned().unwrap_or_default();
    let v = versions
        .iter()
        .find(|v| v["version"] == json!(version))
        .unwrap_or_else(|| panic!("{package}@{version} not listed: {p}"));
    v["services"][""].as_str().unwrap().to_owned()
}

/// The bundle `app` runs, as its owner reads it.
async fn running(owner: &Client, app: &str) -> String {
    owner.apps().get(app).await.map(wire).unwrap()["bundle"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn code(e: &calimero_client::Error) -> Option<&str> {
    e.code()
}

/// The owner makes an upgrade key with a passkey; CI then upgrades that app
/// to a signed release of its package with the key alone, and the directory
/// records the key. The key reads nothing, calls nothing, installs nothing,
/// makes no app or key and upgrades no other app; a raw bundle and another
/// signer's package are refused; once revoked, it upgrades nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upgrade_key_upgrades_its_app_and_does_nothing_else() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let kv = kv_wasm();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let stranger = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
    for (version, wasm) in [
        ("1.0.0", kv.clone()),
        ("1.1.0", variant(&kv, "one-one")),
        ("1.2.0", variant(&kv, "one-two")),
    ] {
        let _ = alice
            .packages()
            .upload(mpk("com.test.ci", version, &wasm, &publisher))
            .await
            .unwrap();
    }
    let _ = alice
        .packages()
        .upload(mpk(
            "com.test.other",
            "1.0.0",
            &variant(&kv, "other"),
            &stranger,
        ))
        .await
        .map(wire)
        .unwrap();
    let create = |name: &str| {
        let alice = &alice;
        let name = name.to_owned();
        async move {
            alice
                .apps()
                .create(typed(
                    json!({ "name": name, "package": "com.test.ci", "version": "1.0.0" }),
                ))
                .await
                .map(wire)
                .unwrap()["appId"]
                .as_str()
                .unwrap()
                .to_owned()
        }
    };
    let app = create("ci-target").await;
    let other_app = create("not-the-target").await;

    // Only the owner, with a passkey, makes it; it carries no other scope.
    let e = alice
        .auth()
        .apikey_create(
            typed(json!({ "upgrades": { "app": app }, "methods": ["set"] })),
            Some(&mut apk),
        )
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap_err();
    assert_eq!(code(&e), Some("BAD_UPGRADE_KEY"), "{e}");
    let (key_id, key) = alice
        .auth()
        .apikey_create(
            typed(json!({ "label": "ci", "upgrades": { "app": app } })),
            Some(&mut apk),
        )
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let keys = alice.auth().apikey_list().await.map(wire).unwrap();
    let listed = keys["keys"]
        .as_array()
        .unwrap()
        .iter()
        .find(|k| k["keyId"] == json!(key_id))
        .cloned()
        .unwrap();
    assert_eq!(listed["upgrades"]["app"], json!(app), "{listed}");
    assert_eq!(
        listed["upgrades"]["package"],
        json!("com.test.ci"),
        "{listed}"
    );
    assert_eq!(
        listed["upgrades"]["signer"],
        json!(calimero_bundle::derive_signer_id_did_key(
            publisher.verifying_key().as_bytes()
        )),
        "{listed}"
    );

    // CI: the key alone, on another node, no passkey.
    let ci = c.client(1);
    ci.use_api_key(&key).await;
    let _ = ci
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.1.0" })), None)
        .await
        .unwrap();
    assert_eq!(
        running(&alice, &app).await,
        bundle_of(&alice, "com.test.ci", "1.1.0").await
    );
    let report = alice
        .auth()
        .account_apps(None, None)
        .await
        .map(wire)
        .unwrap();
    let entry = report["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["app"] == json!(app))
        .cloned()
        .unwrap();
    assert_eq!(entry["lastUpgrade"]["apiKey"], json!(key_id), "{entry}");
    assert_eq!(
        entry["lastUpgrade"]["actor"],
        json!(account_id(&alice).await),
        "{entry}"
    );

    // Nothing else: reads, calls, installs, apps, keys, other apps.
    let only = |r: Result<Value, calimero_client::Error>, what: &str| {
        let e = r.expect_err(what);
        assert_eq!(code(&e), Some("UPGRADE_KEY_ONLY"), "{what}: {e}");
    };
    only(ci.auth().me().await.map(wire), "auth/me");
    only(ci.apps().get(&app).await.map(wire), "read the app");
    only(ci.apps().list(None, None).await.map(wire), "list apps");
    only(
        ci.apps()
            .call(
                &app,
                "set",
                Some(json!({ "key": "k", "value": "v" })),
                None,
                None,
            )
            .await
            .map(|_| Value::Null),
        "call the app",
    );
    only(
        ci.apps()
            .create(typed(
                json!({ "name": "mine", "package": "com.test.ci", "version": "1.0.0" }),
            ))
            .await
            .map(wire),
        "create an app",
    );
    only(
        ci.packages()
            .upload(mpk(
                "com.test.ci",
                "9.0.0",
                &variant(&kv, "nine"),
                &publisher,
            ))
            .await
            .map(wire),
        "install a package",
    );
    only(
        ci.auth()
            .apikey_create(typed(json!({})), None)
            .await
            .map(wire),
        "make a key",
    );
    only(
        ci.apps().transfer(&app, "someone", None).await.map(wire),
        "transfer the app",
    );
    let e = ci
        .apps()
        .upgrade(&other_app, typed(json!({ "version": "1.1.0" })), None)
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("FORBIDDEN"), "another app: {e}");

    // Only signed releases of its package by its signer.
    let raw = bundle_of(&alice, "com.test.ci", "1.2.0").await;
    let e = ci
        .apps()
        .upgrade(&app, typed(json!({ "bundle": raw })), None)
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("FORBIDDEN"), "a raw bundle: {e}");
    let e = ci
        .apps()
        .upgrade(
            &app,
            typed(json!({ "package": "com.test.other", "version": "1.0.0" })),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("FORBIDDEN"), "another signer's package: {e}");
    assert_eq!(
        running(&alice, &app).await,
        bundle_of(&alice, "com.test.ci", "1.1.0").await,
        "nothing refused moved the app"
    );

    // The owner's own upgrades still need a passkey.
    let e = alice
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.2.0" })), None)
        .await
        .unwrap_err();
    assert_eq!(code(&e), Some("CONFIRM_REQUIRED"), "{e}");

    // Revoked: refused at once, on every node.
    let _ = alice
        .auth()
        .apikey_revoke(&key_id, Some(&mut apk))
        .await
        .map(wire)
        .unwrap();
    for i in 0..3 {
        let ci = c.client(i);
        ci.use_api_key(&key).await;
        let e = ci
            .apps()
            .upgrade(&app, typed(json!({ "version": "1.2.0" })), None)
            .await
            .unwrap_err();
        assert!(
            matches!(e, calimero_client::Error::Api { status: 401, .. }),
            "node {i}: {e}"
        );
    }
    assert_eq!(
        running(&alice, &app).await,
        bundle_of(&alice, "com.test.ci", "1.1.0").await
    );
}

/// What applies an upgrade key's upgrade judges it again, from the log
/// alone: an `UpgradeApp` naming the key but signed with anything other than
/// the key, to another package's bundle, or after the key was revoked, is
/// refused on every node, whoever proposed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upgrade_keys_proof_is_judged_where_it_applies() {
    let c = Cluster::start(3).await;
    let (alice, mut apk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_id(&alice).await;
    let kv = kv_wasm();
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let stranger = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
    let _ = alice
        .packages()
        .upload(mpk("com.test.ci", "1.0.0", &kv, &publisher))
        .await
        .unwrap();
    let _ = alice
        .packages()
        .upload(mpk(
            "com.test.ci",
            "1.1.0",
            &variant(&kv, "next"),
            &publisher,
        ))
        .await
        .unwrap();
    let _ = alice
        .packages()
        .upload(mpk(
            "com.test.other",
            "1.0.0",
            &variant(&kv, "other"),
            &stranger,
        ))
        .await
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "ci-target", "package": "com.test.ci", "version": "1.0.0" }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let (key_id, key) = alice
        .auth()
        .apikey_create(typed(json!({ "upgrades": { "app": app } })), Some(&mut apk))
        .await
        .map(|k| (k.key_id, k.key))
        .unwrap();
    let next = bundle_of(&alice, "com.test.ci", "1.1.0").await;
    let foreign = bundle_of(&alice, "com.test.other", "1.0.0").await;
    let upgrade = |bundle: &str, secret: &str| {
        let statement = calimero_node::proof::Statement::new(
            &app,
            "__upgrade",
            &json!({ "bundle": bundle, "migrate": null, "manifest": null }),
        );
        serde_json::to_value(SystemCmd::UpgradeApp {
            app_id: app.clone(),
            bundle: bundle.to_owned(),
            compute_bundle: None,
            by_operator: false,
            actor: alice_id.clone(),
            at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            run: None,
            consent: json!(calimero_node::proof::sign_key_upgrade(
                secret, &key_id, statement
            )),
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        })
        .unwrap()
    };
    let e = append(&c, &upgrade(&next, "cal_not-the-key"))
        .await
        .unwrap_err();
    assert!(e.contains("not signed with the API key"), "{e}");
    let e = append(&c, &upgrade(&foreign, &key)).await.unwrap_err();
    assert!(e.contains("only to signed releases of its package"), "{e}");
    let _ = alice
        .auth()
        .apikey_revoke(&key_id, Some(&mut apk))
        .await
        .unwrap();
    let e = append(&c, &upgrade(&next, &key)).await.unwrap_err();
    assert!(e.contains("revoked"), "{e}");
    assert_ne!(running(&alice, &app).await, next);
}
