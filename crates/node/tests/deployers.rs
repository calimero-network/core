//! Deployer grants: where a cluster's deployments are `granted`, only the
//! accounts someone granted install apps (packages from the registry or an
//! upload, raw bundles); everyone else still signs up and creates instances
//! of the releases they see. The operator grants deployers, or, where the
//! cluster's owner does (`deployers: owner`), the owner once the operator
//! names it.
//!
//! Needs `kv_store.wasm`: `cargo build -p kv-store --target wasm32-unknown-unknown --profile app-release`,
//! or set `KV_STORE_WASM`.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use calimero_client::Client;
use calimero_node::system::{Deployers, Deployments, SystemCmd};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, ops_signed, signed_up, Cluster, OPERATOR_SEED};

/// A signed single-service `.mpk`, the shape `cargo mero bundle` produces.
fn mpk(package: &str, version: &str, wasm: &[u8]) -> Vec<u8> {
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

/// A registry serving `{base}/artifacts/{package}/{version}/{file}`, counting
/// what it is asked for.
#[derive(Clone, Default)]
struct Registry {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    hits: Arc<Mutex<usize>>,
}

impl Registry {
    fn publish(&self, package: &str, version: &str) {
        let _prev = self.files.lock().unwrap().insert(
            format!("{package}@{version}"),
            mpk(package, version, &kv_wasm()),
        );
    }

    fn hits(&self) -> usize {
        *self.hits.lock().unwrap()
    }

    async fn serve(&self) -> String {
        use axum::extract::Path;
        use axum::http::StatusCode;
        use axum::response::IntoResponse;
        let reg = self.clone();
        let app = axum::Router::new().route(
            "/artifacts/{package}/{version}/{file}",
            axum::routing::get(
                move |Path((package, version, _file)): Path<(String, String, String)>| {
                    let reg = reg.clone();
                    async move {
                        *reg.hits.lock().unwrap() += 1;
                        match reg
                            .files
                            .lock()
                            .unwrap()
                            .get(&format!("{package}@{version}"))
                        {
                            Some(b) => (StatusCode::OK, b.clone()).into_response(),
                            None => StatusCode::NOT_FOUND.into_response(),
                        }
                    }
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let _serving = tokio::spawn(async move { axum::serve(listener, app).await });
        format!("http://{addr}/")
    }
}

/// The client's account id.
async fn account_of(client: &Client) -> String {
    client.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A client holding the ops token.
async fn ops(c: &Cluster) -> Client {
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    ops
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

/// The operator key's signature over `cluster / method / args` in `c`, as
/// an `operatorProof` body field signs it (`nonce` of our own).
fn operator_proof(c: &Cluster, seed: [u8; 32], method: &str, args: &Value) -> Value {
    use ed25519_dalek::Signer;
    let statement = calimero_node::proof::Statement::new("cluster", method, args);
    let signature = ed25519_dalek::SigningKey::from_bytes(&seed)
        .sign(&statement.operator_message(&c.name))
        .to_bytes();
    json!({ "nonce": statement.nonce, "signature": hex::encode(signature) })
}

/// The same signature as the log carries it (`proof` of a system command).
fn logged_proof(c: &Cluster, seed: [u8; 32], method: &str, args: &Value) -> Value {
    use ed25519_dalek::Signer;
    let statement = calimero_node::proof::Statement::new("cluster", method, args);
    let signature = ed25519_dalek::SigningKey::from_bytes(&seed)
        .sign(&statement.operator_message(&c.name))
        .to_bytes();
    serde_json::to_value(calimero_node::proof::OperatorProof {
        statement,
        signature: hex::encode(signature),
    })
    .unwrap()
}

fn refused(e: &calimero_client::Error) {
    assert_eq!(e.code(), Some("DEPLOY_NOT_GRANTED"), "{e}");
    assert!(e.to_string().contains("deployer grant"), "{e}");
}

/// On a granted cluster an account without a grant installs nothing: the API
/// refuses before it fetches, verifies or stores anything, and an install
/// logged anyway is refused where it applies. The operator's signed grant
/// lets it (a grant signed by another key, or sent again, does not); its
/// revocation stops new installs while its apps keep running and everyone
/// still creates instances of the releases they see.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn only_granted_accounts_install_on_a_granted_cluster() {
    let registry = Registry::default();
    let url = registry.serve().await;
    for v in ["1.0.0", "1.1.0", "1.2.0"] {
        registry.publish("com.test.kv", v);
    }
    registry.publish("com.test.catalog", "1.0.0");
    let c = Cluster::start_with(3, |cfg| {
        cfg.registry = Some(url.clone());
        cfg.governance.deployments = Deployments::Granted;
    })
    .await;
    let ops = ops(&c).await;
    let (alice, _alice_pk) = signed_up(&c, 0, "alice").await;
    let alice_id = account_of(&alice).await;
    let (bob, _bob_pk) = signed_up(&c, 1, "bob").await;

    // No grant: refused before any work, through every way in.
    refused(
        &alice
            .install_package("com.test.kv", "1.0.0")
            .await
            .unwrap_err(),
    );
    assert_eq!(registry.hits(), 0, "refused before the registry is asked");
    refused(
        &alice
            .upload_package(mpk("com.test.up", "1.0.0", &kv_wasm()))
            .await
            .unwrap_err(),
    );
    refused(&alice.upload_bundle(kv_wasm()).await.unwrap_err());
    refused(
        &alice
            .create_app_from_package("kv", "com.test.kv", Some("1.0.0"), None, json!({}))
            .await
            .unwrap_err(),
    );
    assert_eq!(registry.hits(), 0);
    // Logged anyway (a node's own code): refused where it applies.
    let forged = serde_json::to_value(SystemCmd::InstallPackage {
        package: "com.test.kv".into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value(json!({
            "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "registry",
            "services": { "": "00" }, "installedBy": alice_id, "installedAt": 1,
        }))
        .unwrap(),
    })
    .unwrap();
    let e = append(&c, &forged).await.unwrap_err();
    assert!(e.contains("deployer grant"), "{e}");
    let raw = serde_json::to_value(SystemCmd::PutBundle {
        hash: "00".into(),
        size: 1,
        actor: alice_id.clone(),
        at: 1,
    })
    .unwrap();
    let e = append(&c, &raw).await.unwrap_err();
    assert!(e.contains("deployer grant"), "{e}");

    // The operator's grant: the token alone gets the bytes to sign; another
    // key's signature is refused; the operator's applies, once.
    let body = json!({ "deployer": { "account": alice_id } });
    let e = ops.post("ops/deployers", body.clone()).await.unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let args = json!({ "deployer": { "account": alice_id }, "until": null });
    let mut other = body.clone();
    other["operatorProof"] = operator_proof(&c, [8; 32], "__grant_deployer", &args);
    let e = ops.post("ops/deployers", other).await.unwrap_err();
    assert_eq!(e.code(), Some("BAD_OPERATOR_SIGNATURE"), "{e}");
    let mut signed = body.clone();
    signed["operatorProof"] = operator_proof(&c, OPERATOR_SEED, "__grant_deployer", &args);
    let _ok = ops.post("ops/deployers", signed.clone()).await.unwrap();
    let e = ops.post("ops/deployers", signed).await.unwrap_err();
    assert!(e.to_string().contains("used already"), "{e}");

    let _ok = alice.install_package("com.test.kv", "1.0.0").await.unwrap();
    let app = alice
        .create_app_from_package("kv", "com.test.kv", Some("1.0.0"), None, json!({}))
        .await
        .unwrap();
    let _ok = alice
        .call(&app, "set", json!({ "key": "k", "value": "v1" }), None)
        .await
        .unwrap();

    // Customers: an instance of a release they see (the operator's catalog)
    // needs no grant; one the registry would install first does.
    let _ok = ops
        .ops_install_package("com.test.catalog", "1.0.0")
        .await
        .unwrap();
    let _app = bob
        .create_app_from_package("cat", "com.test.catalog", Some("1.0.0"), None, json!({}))
        .await
        .unwrap();
    refused(
        &bob.create_app_from_package("kv", "com.test.kv", Some("1.1.0"), None, json!({}))
            .await
            .unwrap_err(),
    );

    // Only the operator (and the owner) read the grants.
    let listed = ops.get("deployers").await.unwrap();
    assert_eq!(listed["deployments"], "granted");
    assert_eq!(listed["deployers"], "operator");
    assert_eq!(
        listed["grants"][0]["deployer"],
        json!({ "account": alice_id })
    );
    let e = bob.get("deployers").await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    // Revoked: no new install; her app runs, and she still creates
    // instances of the release she installed.
    let path = format!("ops/deployers/account:{alice_id}");
    let e = ops.delete(&path).await.unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let proof = operator_proof(
        &c,
        OPERATOR_SEED,
        "__revoke_deployer",
        &json!({ "deployer": { "account": alice_id } }),
    );
    let _ok = ops
        .delete_with(&path, json!({ "operatorProof": proof }))
        .await
        .unwrap();
    refused(
        &alice
            .install_package("com.test.kv", "1.2.0")
            .await
            .unwrap_err(),
    );
    let _ok = alice
        .call(&app, "set", json!({ "key": "k", "value": "v2" }), None)
        .await
        .unwrap();
    let _again = alice
        .create_app_from_package("kv2", "com.test.kv", Some("1.0.0"), None, json!({}))
        .await
        .unwrap();
}

/// A cluster deployed with the defaults behaves as before and attests the
/// governance it did; the operator turns its deployments granted once (an
/// existing cluster need not be recreated), and every receipt key attests the
/// new rule.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_open_cluster_installs_as_before_until_the_operator_grants_deployments() {
    let c = Cluster::start(3).await;
    let ops = ops(&c).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let app = alice.create_app("kv", &bundle, json!({})).await.unwrap();
    let rules = alice.get("releases").await.unwrap();
    assert_eq!(
        rules["governance"],
        json!({ "releases": "owner", "appUpgrades": "owner", "outbound": "owner" })
    );
    let proven = alice.verify_app_cluster(&app).await.unwrap();
    assert_eq!(proven.governance.deployments, Deployments::Open);

    let e = ops
        .post("ops/deployments/granted", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let granted = |seed| json!({ "operatorProof": operator_proof(&c, seed, "__deployments_granted", &json!({})) });
    let _ok = ops
        .post("ops/deployments/granted", granted(OPERATOR_SEED))
        .await
        .unwrap();
    let e = ops
        .post("ops/deployments/granted", granted(OPERATOR_SEED))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFLICT"), "one way, once: {e}");
    // Logged directly again: refused where it applies.
    let again = serde_json::to_value(SystemCmd::SetDeploymentsGranted {
        proof: logged_proof(&c, OPERATOR_SEED, "__deployments_granted", &json!({})),
    })
    .unwrap();
    assert!(append(&c, &again).await.is_err());

    refused(&alice.upload_bundle(kv_wasm()).await.unwrap_err());
    let _instance = alice.create_app("kv2", &bundle, json!({})).await.unwrap();
    let rules = alice.get("releases").await.unwrap();
    assert_eq!(rules["governance"]["deployments"], "granted");
    // Every replica attests it once it has applied the change.
    let mut proven = None;
    for _ in 0..30 {
        if let Ok(p) = alice.verify_app_cluster(&app).await {
            if p.governance.deployments == Deployments::Granted {
                proven = Some(p);
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(proven.is_some(), "the replicas attest granted deployments");
}

/// Where the cluster's owner grants deployers: the operator grants them until
/// it names the owner, once; then the operator's grant is refused (at the API
/// and where it applies) and the owner's passkey grants and revokes them.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_cluster_owner_grants_deployers_once_the_operator_names_it() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance.deployments = Deployments::Granted;
        cfg.governance.deployers = Deployers::Owner;
    })
    .await;
    let ops = ops(&c).await;
    let (owner, mut owner_pk) = signed_up(&c, 0, "owner").await;
    let owner_id = account_of(&owner).await;
    let (dev, _dev_pk) = signed_up(&c, 1, "dev").await;
    let dev_id = account_of(&dev).await;
    let (other, mut other_pk) = signed_up(&c, 2, "other").await;
    let other_id = account_of(&other).await;

    // Before the claim: the operator grants; the owner-to-be cannot.
    let _ok = ops_signed(
        &ops,
        "ops/deployers",
        json!({ "deployer": { "account": other_id } }),
    )
    .await
    .unwrap();
    let e = owner
        .post_confirmed(
            "deployers",
            json!({ "deployer": { "account": dev_id } }),
            &mut owner_pk,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    // The operator names the owner, once.
    let _ok = ops_signed(
        &ops,
        "ops/cluster-owner",
        json!({ "owner": { "account": owner_id } }),
    )
    .await
    .unwrap();
    let e = ops_signed(
        &ops,
        "ops/cluster-owner",
        json!({ "owner": { "account": dev_id } }),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code(), Some("CONFLICT"), "{e}");

    // The operator's grant: refused at the API and where it applies.
    let e = ops_signed(
        &ops,
        "ops/deployers",
        json!({ "deployer": { "account": dev_id } }),
    )
    .await
    .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let forged = serde_json::to_value(SystemCmd::GrantDeployer {
        deployer: calimero_node::system::Deployer::Account(dev_id.clone()),
        until: None,
        at: 1,
        actor: calimero_node::system::OPERATOR.into(),
        proof: Some(logged_proof(
            &c,
            OPERATOR_SEED,
            "__grant_deployer",
            &json!({ "deployer": { "account": dev_id }, "until": null }),
        )),
        consent: None,
    })
    .unwrap();
    assert!(append(&c, &forged).await.is_err());
    refused(&dev.upload_bundle(kv_wasm()).await.unwrap_err());

    // Someone else's passkey grants nothing; the owner's does.
    let e = other
        .post_confirmed(
            "deployers",
            json!({ "deployer": { "account": dev_id } }),
            &mut other_pk,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let _ok = owner
        .post_confirmed(
            "deployers",
            json!({ "deployer": { "account": dev_id } }),
            &mut owner_pk,
        )
        .await
        .unwrap();
    let _hash = dev.upload_bundle(kv_wasm()).await.unwrap();
    let listed = owner.get("deployers").await.unwrap();
    assert_eq!(listed["owner"], json!({ "account": owner_id }));
    assert_eq!(listed["grants"].as_array().unwrap().len(), 2);

    let _ok = owner
        .delete_confirmed(&format!("deployers/account:{dev_id}"), &mut owner_pk)
        .await
        .unwrap();
    refused(&dev.upload_bundle(kv_wasm()).await.unwrap_err());
}
