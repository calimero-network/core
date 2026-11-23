//! Deployer grants: where a cluster's deployments are `granted`, only the
//! accounts someone granted install apps (packages from the registry or an
//! upload, raw bundles); everyone else still signs up and creates instances
//! of the releases they see. The operator grants deployers, or, where the
//! cluster's owner does (`deployers: owner`), the owner once the operator
//! names it.
//!
//! Builds the component kv fixture `apps/fixture-kv` from this checkout
//! (see `harness/cluster.rs`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use calimero_client::{Client, Request};
use calimero_node::system::{Deployers, Deployments, SystemCmd};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, ops_signed, signed_up, typed, wire, Cluster, OPERATOR_SEED};
use reqwest::Method;

/// A signed single-service `.mpk`, the shape `meroctl app pack` signs.
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
        compute: None,
        links: None,
        frontend: None,
        frontend_aliases: Vec::new(),
        upgrades: None,
        instance_org: None,
        guests: None,
        outbound: Vec::new(),
        secrets: None,
        uses: None,
        services: None,
        pipelines: Vec::new(),
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

/// The cluster's registry credential, as `--registry-url` carries it, and the
/// `Authorization` it sends.
const REGISTRY_USER: &str = "cluster:secret";
const REGISTRY_AUTH: &str = "Basic Y2x1c3RlcjpzZWNyZXQ=";

/// One fetch the registry was asked for: `package@version`, whether it carried the
/// credential, and the account it named.
type Asked = (String, bool, Option<String>);

/// A registry serving `{base}/artifacts/{package}/{version}/{file}`, counting
/// what it is asked for. A public release is served to anyone; a private one
/// only with the credential, to an account allowed it (as mdma's authorize does),
/// and then again with the credential alone (another node's re-fetch).
#[derive(Clone, Default)]
struct Registry {
    files: Arc<Mutex<BTreeMap<String, Vec<u8>>>>,
    hits: Arc<Mutex<usize>>,
    private: Arc<Mutex<BTreeMap<String, Vec<String>>>>,
    authorized: Arc<Mutex<Vec<String>>>,
    asked: Arc<Mutex<Vec<Asked>>>,
}

impl Registry {
    /// Publishes `package@version`, its WASM tagged with the package (a custom section),
    /// so no two packages share one and each node fetches each package's.
    fn publish(&self, package: &str, version: &str) {
        let mut wasm = kv_wasm();
        wasm.extend([0, u8::try_from(1 + package.len()).unwrap()]);
        wasm.push(u8::try_from(package.len()).unwrap());
        wasm.extend_from_slice(package.as_bytes());
        let _prev = self
            .files
            .lock()
            .unwrap()
            .insert(format!("{package}@{version}"), mpk(package, version, &wasm));
    }

    /// Publishes `package@version` as private, for `accounts` only.
    fn publish_private(&self, package: &str, version: &str, accounts: &[&str]) {
        self.publish(package, version);
        let _prev = self.private.lock().unwrap().insert(
            format!("{package}@{version}"),
            accounts.iter().map(|a| (*a).to_owned()).collect(),
        );
    }

    fn hits(&self) -> usize {
        *self.hits.lock().unwrap()
    }

    fn asked(&self) -> Vec<Asked> {
        self.asked.lock().unwrap().clone()
    }

    fn serves(&self, key: &str, credential: bool, account: Option<&str>) -> bool {
        let Some(allowed) = self.private.lock().unwrap().get(key).cloned() else {
            return true;
        };
        let mut authorized = self.authorized.lock().unwrap();
        match account {
            _ if !credential => false,
            Some(a) if allowed.iter().any(|x| x == a) => {
                authorized.push(key.to_owned());
                true
            }
            Some(_) => false,
            None => authorized.iter().any(|k| k == key),
        }
    }

    /// Serves on 127.0.0.1; the URL carries the credential.
    async fn serve(&self) -> String {
        use axum::extract::Path;
        use axum::http::{HeaderMap, StatusCode};
        use axum::response::IntoResponse;
        let reg = self.clone();
        let app = axum::Router::new().route(
            "/artifacts/{package}/{version}/{file}",
            axum::routing::get(
                move |Path((package, version, _file)): Path<(String, String, String)>,
                      headers: HeaderMap| {
                    let reg = reg.clone();
                    async move {
                        *reg.hits.lock().unwrap() += 1;
                        let key = format!("{package}@{version}");
                        let credential = headers
                            .get("authorization")
                            .is_some_and(|v| v == REGISTRY_AUTH);
                        let account = headers
                            .get("x-calimero-install-account")
                            .and_then(|v| v.to_str().ok())
                            .map(str::to_owned);
                        reg.asked
                            .lock()
                            .unwrap()
                            .push((key.clone(), credential, account.clone()));
                        if !reg.serves(&key, credential, account.as_deref()) {
                            return StatusCode::NOT_FOUND.into_response();
                        }
                        match reg.files.lock().unwrap().get(&key) {
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
        format!("http://{REGISTRY_USER}@{addr}/")
    }
}

/// The client's account id.
async fn account_of(client: &Client) -> String {
    client.auth().me().await.map(wire).unwrap()["accountId"]
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
            .packages()
            .install("com.test.kv", "1.0.0", None)
            .await
            .unwrap_err(),
    );
    assert_eq!(registry.hits(), 0, "refused before the registry is asked");
    refused(
        &alice
            .packages()
            .upload(mpk("com.test.up", "1.0.0", &kv_wasm()), None)
            .await
            .unwrap_err(),
    );
    refused(
        &alice
            .packages()
            .bundle_put(kv_wasm())
            .await
            .map(|b| b.hash)
            .unwrap_err(),
    );
    refused(
        &alice
            .apps()
            .create(typed(
                json!({ "name": "kv", "package": "com.test.kv", "version": "1.0.0" }),
            ))
            .await
            .map(|a| a.app_id)
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
    let e = ops
        .transport()
        .json(Request::new(Method::POST, "ops/deployers").json(body.clone()))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let args = json!({ "deployer": { "account": alice_id }, "until": null });
    let mut other = body.clone();
    other["operatorProof"] = operator_proof(&c, [8; 32], "__grant_deployer", &args);
    let e = ops
        .transport()
        .json(Request::new(Method::POST, "ops/deployers").json(other))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_OPERATOR_SIGNATURE"), "{e}");
    let mut signed = body.clone();
    signed["operatorProof"] = operator_proof(&c, OPERATOR_SEED, "__grant_deployer", &args);
    let _ok = ops
        .transport()
        .json(Request::new(Method::POST, "ops/deployers").json(signed.clone()))
        .await
        .unwrap();
    let e = ops
        .transport()
        .json(Request::new(Method::POST, "ops/deployers").json(signed))
        .await
        .unwrap_err();
    assert!(e.to_string().contains("used already"), "{e}");

    let _ok = alice
        .packages()
        .install("com.test.kv", "1.0.0", None)
        .await
        .unwrap();
    let app = alice
        .apps()
        .create(typed(
            json!({ "name": "kv", "package": "com.test.kv", "version": "1.0.0" }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let _ok = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v1" })),
            None,
            None,
        )
        .await
        .unwrap();

    // Customers: an instance of a release they see (the operator's catalog)
    // needs no grant; one the registry would install first does.
    let _ok = ops
        .ops()
        .package_install("com.test.catalog", "1.0.0")
        .await
        .map(wire)
        .unwrap();
    let _app = bob
        .apps()
        .create(typed(
            json!({ "name": "cat", "package": "com.test.catalog", "version": "1.0.0" }),
        ))
        .await
        .map(|a| a.app_id)
        .unwrap();
    refused(
        &bob.apps()
            .create(typed(
                json!({ "name": "kv", "package": "com.test.kv", "version": "1.1.0" }),
            ))
            .await
            .map(|a| a.app_id)
            .unwrap_err(),
    );

    // Only the operator (and the owner) read the grants.
    let listed = ops.deployments().deployer_list().await.map(wire).unwrap();
    assert_eq!(listed["deployments"], "granted");
    assert_eq!(listed["deployers"], "operator");
    assert_eq!(
        listed["grants"][0]["deployer"],
        json!({ "account": alice_id })
    );
    let e = bob.deployments().deployer_list().await.unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    // Revoked: no new install; her app runs, and she still creates
    // instances of the release she installed.
    let path = format!("ops/deployers/account:{alice_id}");
    let e = ops
        .ops()
        .deployer_revoke_delete(&format!("account:{alice_id}"), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let proof = operator_proof(
        &c,
        OPERATOR_SEED,
        "__revoke_deployer",
        &json!({ "deployer": { "account": alice_id } }),
    );
    // The POST form, for a client whose DELETE carries no body.
    let _ok = ops
        .transport()
        .json(
            Request::new(Method::POST, format!("{path}/revoke"))
                .json(json!({ "operatorProof": proof })),
        )
        .await
        .unwrap();
    refused(
        &alice
            .packages()
            .install("com.test.kv", "1.2.0", None)
            .await
            .unwrap_err(),
    );
    let _ok = alice
        .apps()
        .call(
            &app,
            "set",
            Some(json!({ "key": "k", "value": "v2" })),
            None,
            None,
        )
        .await
        .unwrap();
    let _again = alice
        .apps()
        .create(typed(
            json!({ "name": "kv2", "package": "com.test.kv", "version": "1.0.0" }),
        ))
        .await
        .map(|a| a.app_id)
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
    let bundle = alice
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let rules = alice.public().releases_public().await.map(wire).unwrap();
    assert_eq!(
        rules["governance"],
        json!({ "releases": "owner", "appUpgrades": "owner", "outbound": "owner" })
    );
    let proven = alice.verify_app_cluster(&app).await.unwrap();
    assert_eq!(proven.governance.deployments, Deployments::Open);

    let e = ops.ops().deployments_granted(None).await.unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let granted = |seed| json!({ "operatorProof": operator_proof(&c, seed, "__deployments_granted", &json!({})) });
    let _ok = ops
        .transport()
        .json(Request::new(Method::POST, "ops/deployments/granted").json(granted(OPERATOR_SEED)))
        .await
        .unwrap();
    let e = ops
        .transport()
        .json(Request::new(Method::POST, "ops/deployments/granted").json(granted(OPERATOR_SEED)))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFLICT"), "one way, once: {e}");
    // Logged directly again: refused where it applies.
    let again = serde_json::to_value(SystemCmd::SetDeploymentsGranted {
        proof: logged_proof(&c, OPERATOR_SEED, "__deployments_granted", &json!({})),
    })
    .unwrap();
    assert!(append(&c, &again).await.is_err());

    refused(
        &alice
            .packages()
            .bundle_put(kv_wasm())
            .await
            .map(|b| b.hash)
            .unwrap_err(),
    );
    let _instance = alice
        .apps()
        .create(typed(json!({ "name": "kv2", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let rules = alice.public().releases_public().await.map(wire).unwrap();
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
        .deployments()
        .deployer_grant(
            typed(json!({ "account": dev_id })),
            None,
            None,
            Some(&mut owner_pk),
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
    refused(
        &dev.packages()
            .bundle_put(kv_wasm())
            .await
            .map(|b| b.hash)
            .unwrap_err(),
    );

    // Someone else's passkey grants nothing; the owner's does.
    let e = other
        .deployments()
        .deployer_grant(
            typed(json!({ "account": dev_id })),
            None,
            None,
            Some(&mut other_pk),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");
    let _ok = owner
        .deployments()
        .deployer_grant(
            typed(json!({ "account": dev_id })),
            None,
            None,
            Some(&mut owner_pk),
        )
        .await
        .unwrap();
    let _hash = dev
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .unwrap();
    let listed = owner.deployments().deployer_list().await.map(wire).unwrap();
    assert_eq!(listed["owner"], json!({ "account": owner_id }));
    assert_eq!(listed["grants"].as_array().unwrap().len(), 2);

    let _ok = owner
        .deployments()
        .deployer_revoke(&format!("account:{dev_id}"), Some(&mut owner_pk))
        .await
        .unwrap();
    refused(
        &dev.packages()
            .bundle_put(kv_wasm())
            .await
            .map(|b| b.hash)
            .unwrap_err(),
    );
}

/// On a shared, granted cluster an account of another org must not get org A's
/// private package by installing it from the operator's registry: the registry
/// serves it only to the account its publisher allowed (alice's), with the
/// cluster's credential, and the release is used only in the org it was installed for.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn another_orgs_account_cannot_install_a_private_package_from_the_registry() {
    let registry = Registry::default();
    let url = registry.serve().await;
    let c = Cluster::start_with(3, |cfg| {
        cfg.registry = Some(url.clone());
        cfg.governance.deployments = Deployments::Granted;
    })
    .await;
    let ops = ops(&c).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let alice_id = account_of(&alice).await;
    registry.publish_private("com.acme.secret", "1.0.0", &[&alice_id]);
    registry.publish_private("com.acme.secret2", "1.0.0", &[&alice_id]);
    for who in [&alice, &bob] {
        let _ok = ops_signed(
            &ops,
            "ops/deployers",
            json!({ "deployer": { "account": account_of(who).await } }),
        )
        .await
        .unwrap();
    }
    let _acme = alice.orgs().create("Acme", None, None).await.unwrap();
    let _bobco = bob.orgs().create("Bobco", None, None).await.unwrap();
    let mut alices_app = String::new();
    for p in ["com.acme.secret", "com.acme.secret2"] {
        let _ok = alice.packages().install(p, "1.0.0", None).await.unwrap();
        alices_app = alice
            .apps()
            .create(typed(
                json!({ "name": "a", "package": p, "version": "1.0.0" }),
            ))
            .await
            .map(|a| a.app_id)
            .unwrap();
    }

    let hidden = |what: &str, e: calimero_client::Error| {
        assert!(
            matches!(
                e,
                calimero_client::Error::Api {
                    status: 403 | 404,
                    ..
                }
            ),
            "{what}: {e}"
        );
    };
    let p = bob.packages();
    hidden("control get", p.get("com.acme.secret").await.unwrap_err());
    hidden(
        "control get 2",
        p.get("com.acme.secret2").await.unwrap_err(),
    );
    hidden(
        "control alice's app",
        bob.apps().get(&alices_app).await.unwrap_err(),
    );

    let mut leaks = Vec::new();
    match p.install("com.acme.secret", "1.0.0", None).await.map(wire) {
        Ok(v) => leaks.push(format!("bob's install from the registry answered 200: {v}")),
        Err(e) => hidden("bob's install", e),
    }
    match p.get("com.acme.secret").await.map(wire) {
        Ok(v) => leaks.push(format!("bob get now 200: {v}")),
        Err(e) => hidden("get after", e),
    }
    for (what, r) in [
        (
            "pipelines",
            p.pipelines("com.acme.secret", "1.0.0").await.map(wire),
        ),
        (
            "capabilities",
            p.capabilities("com.acme.secret", "1.0.0", None)
                .await
                .map(wire),
        ),
    ] {
        if let Ok(v) = r {
            let v = v.to_string();
            leaks.push(format!("bob {what} now 200: {}", &v[..v.len().min(200)]));
        }
    }
    match bob
        .apps()
        .create(typed(
            json!({ "name": "b", "package": "com.acme.secret", "version": "1.0.0" }),
        ))
        .await
        .map(wire)
    {
        Ok(app) => {
            let abi = bob
                .apps()
                .abi(app["appId"].as_str().unwrap())
                .await
                .map(wire);
            let abi = format!("{abi:?}");
            leaks.push(format!(
                "bob created his own instance {app}; its ABI read: {}",
                &abi[..abi.len().min(200)]
            ));
        }
        Err(e) => hidden("bob create after install", e),
    }
    // Create alone, with no explicit install, on a package bob has not touched.
    match bob
        .apps()
        .create(typed(
            json!({ "name": "c", "package": "com.acme.secret2", "version": "1.0.0" }),
        ))
        .await
        .map(wire)
    {
        Ok(app) => leaks.push(format!(
            "bob's create with no install installed from the registry and answered 200: {app}"
        )),
        Err(e) => hidden("bob create without install", e),
    }
    hidden(
        "alice's app stays unreadable",
        bob.apps().get(&alices_app).await.unwrap_err(),
    );
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// A release the log records is fetched again (by a node that runs it and lacks it)
/// naming no account: a private one with the cluster's credential, as the registry
/// serves only what an allowed account installed, and a public one with none.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_release_is_fetched_again_with_the_credential_only_while_it_is_private() {
    let registry = Registry::default();
    let url = registry.serve().await;
    let c = Cluster::start_with(3, |cfg| cfg.registry = Some(url.clone())).await;
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let _acme = alice.orgs().create("Acme", None, None).await.unwrap();
    let alice_id = account_of(&alice).await;
    registry.publish_private("com.acme.secret", "1.0.0", &[&alice_id]);
    registry.publish("com.test.public", "1.0.0");
    // Each node runs the app from its own copy: the other two fetch it again.
    let session = alice.session().await.unwrap();
    for p in ["com.acme.secret", "com.test.public"] {
        let _ok = alice.packages().install(p, "1.0.0", None).await.unwrap();
        let app = alice
            .apps()
            .create(typed(
                json!({ "name": "a", "package": p, "version": "1.0.0" }),
            ))
            .await
            .unwrap()
            .app_id;
        for i in 0..3 {
            let cl = c.client(i);
            cl.use_session(session.clone()).await;
            let _v = cl
                .apps()
                .view(
                    &app,
                    "get",
                    Some(json!({ "key": "k" })),
                    Some(calimero_client::types::ViewsRequestConsistency::Stale),
                    Some(0),
                )
                .await
                .unwrap();
        }
    }

    let asked = |key: &str| -> Vec<(bool, Option<String>)> {
        registry
            .asked()
            .into_iter()
            .filter(|(k, _, _)| k == key)
            .map(|(_, credential, account)| (credential, account))
            .collect()
    };
    let private = asked("com.acme.secret@1.0.0");
    assert_eq!(
        private[..2],
        [(false, None), (true, Some(alice_id.clone()))],
        "the install: without the credential, then with it and the account"
    );
    assert!(
        private.len() > 2 && private[2..].iter().all(|a| *a == (true, None)),
        "{private:?}"
    );
    let public = asked("com.test.public@1.0.0");
    assert!(
        public.len() > 1 && public.iter().all(|a| *a == (false, None)),
        "{public:?}"
    );
}
