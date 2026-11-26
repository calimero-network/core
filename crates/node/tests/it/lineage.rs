//! A release declaring `lineage: pinned` keeps its instances on its package and signer, moving
//! only forward (`SystemState::within_lineage`): an organization's owner moving one to an older
//! release, to another package (their own or the operator's catalog) or to a raw bundle is
//! refused before the passkey is asked for, and the operator is held to it too. Either way the
//! app's group keeps the bundle it ran: the move is refused before `__upgrade` reaches it.
//!
//! Builds the component kv fixture `apps/fixture-kv` from this checkout
//! (see `harness/cluster.rs`).

use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use serde_json::{json, Value};

use crate::harness;

use harness::{kv_wasm, signed_up, typed, wire, Cluster};

const SEALED: &str = "com.test.sealed";
const OTHER: &str = "com.test.other";
const CATALOG: &str = "com.test.catalog";

/// A signed single-service `.mpk` of `package@version`, its WASM tagged so each release ships
/// its own, declaring `lineage: pinned` when `pinned`.
fn release(package: &str, version: &str, pinned: bool) -> Vec<u8> {
    use sha2::Digest;
    let key = ed25519_dalek::SigningKey::from_bytes(&[6; 32]);
    let wasm = tagged(&format!("{package}@{version}"));
    let mut manifest = json!({
        "version": "1.0",
        "package": package,
        "appVersion": version,
        "signerId": calimero_bundle::derive_signer_id_did_key(key.verifying_key().as_bytes()),
        "minRuntimeVersion": "0.1.0",
        "wasm": { "path": "app.wasm", "hash": hex::encode(sha2::Sha256::digest(&wasm)), "size": wasm.len() },
    });
    if pinned {
        manifest["lineage"] = json!("pinned");
    }
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

/// The kv fixture with a custom section named `tag`: another module that runs the same.
fn tagged(tag: &str) -> Vec<u8> {
    let mut wasm = kv_wasm();
    wasm.extend([
        0,
        u8::try_from(1 + tag.len()).unwrap(),
        u8::try_from(tag.len()).unwrap(),
    ]);
    wasm.extend_from_slice(tag.as_bytes());
    wasm
}

fn s(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not a string: {v}"))
        .to_owned()
}

/// How many assertions the passkey has made.
fn assertions(pk: &SoftAuthenticator) -> u64 {
    serde_json::from_str::<Value>(&pk.to_json()).unwrap()["counter"]
        .as_u64()
        .unwrap()
}

/// The bundle `app`'s group runs on node 0.
fn group_bundle(c: &Cluster, app: &str) -> Option<String> {
    c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .hosted(app)
        .unwrap()
        .read_local(|m, _| m.state.bundle.clone())
}

async fn create(owner: &Client, version: &str, org: Option<&str>) -> String {
    let mut body = json!({ "name": "store", "package": SEALED, "version": version });
    if let Some(org) = org {
        body["org"] = json!(org);
    }
    s(&owner.apps().create(typed(body)).await.map(wire).unwrap()["appId"])
}

/// On an open cluster, where the organization's owner is a deployer: an instance of a pinning
/// release records its lineage, and each move off it (an older release, their own package, the
/// operator's catalog, a raw bundle) is refused before the passkey with the group's bundle
/// unchanged; a newer release of the package goes through.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_pinned_instance_moves_only_forward_on_its_own_package() {
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let org = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    for (package, version, pinned) in [
        (SEALED, "1.0.0", true),
        (SEALED, "1.1.0", true),
        (SEALED, "1.2.0", true),
        (OTHER, "1.0.0", false),
    ] {
        let _ok = alice
            .packages()
            .upload(release(package, version, pinned), Some(&org))
            .await
            .unwrap_or_else(|e| panic!("{package}@{version}: {e}"));
    }
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    let _ok = ops
        .ops()
        .package_upload(release(CATALOG, "1.0.0", false))
        .await
        .unwrap();
    let raw = alice
        .packages()
        .bundle_put(tagged("raw"))
        .await
        .map(|b| b.hash)
        .unwrap();

    let app = create(&alice, "1.1.0", Some(&org)).await;

    let moves = [
        (
            "an older release",
            json!({ "version": "1.0.0" }),
            "LINEAGE_OLDER_RELEASE",
        ),
        (
            "their own package",
            json!({ "package": OTHER, "version": "1.0.0" }),
            "OUTSIDE_LINEAGE",
        ),
        (
            "the catalog",
            json!({ "package": CATALOG, "version": "1.0.0" }),
            "OUTSIDE_LINEAGE",
        ),
        ("a raw bundle", json!({ "bundle": raw }), "OUTSIDE_LINEAGE"),
    ];
    let mut moved = Vec::new();
    for (what, body, code) in moves {
        let before = (group_bundle(&c, &app), assertions(&pk));
        match alice.apps().upgrade(&app, typed(body), Some(&mut pk)).await {
            Ok(_) => moved.push(what),
            Err(e) => {
                assert_eq!(e.code(), Some(code), "{what}: {e}");
                assert_eq!(
                    (group_bundle(&c, &app), assertions(&pk)),
                    before,
                    "{what}: refused before the passkey, the group unmoved"
                );
            }
        }
    }
    assert!(moved.is_empty(), "moved off the lineage to {moved:?}");
    let record = alice.apps().get(&app).await.map(wire).unwrap();
    assert_eq!(record["lineage"]["package"], json!(SEALED), "{record}");
    assert!(
        s(&record["lineage"]["signerId"]).starts_with("did:key:"),
        "{record}"
    );

    let up = alice
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.2.0" })), Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
}

/// Where the operator upgrades apps, it is held to the lineage as anyone is: `run_upgrade`
/// refuses the move before the app's group runs it, so the group keeps its bundle.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_operator_cannot_move_a_pinned_instance_off_its_lineage() {
    use calimero_node::system::{Authority, Governance};
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance = Governance {
            app_upgrades: Authority::Operator,
            ..Governance::default()
        };
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    for (package, version, pinned) in [
        (SEALED, "1.0.0", true),
        (SEALED, "1.1.0", true),
        (CATALOG, "1.0.0", false),
    ] {
        let _ok = ops
            .ops()
            .package_upload(release(package, version, pinned))
            .await
            .unwrap_or_else(|e| panic!("{package}@{version}: {e}"));
    }
    let app = create(&alice, "1.1.0", None).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let running = node.app(&app).await.unwrap().bundle;
    assert_eq!(group_bundle(&c, &app).as_ref(), Some(&running));

    let mut moved = Vec::new();
    for (what, body, code) in [
        (
            "an older release",
            json!({ "version": "1.0.0" }),
            "LINEAGE_OLDER_RELEASE",
        ),
        (
            "the catalog",
            json!({ "package": CATALOG, "version": "1.0.0" }),
            "OUTSIDE_LINEAGE",
        ),
    ] {
        match harness::ops_signed(&ops, &format!("ops/apps/{app}/upgrade"), body).await {
            Ok(_) => moved.push(what),
            Err(e) => assert_eq!(e.code(), Some(code), "{what}: {e}"),
        }
        assert_eq!(
            group_bundle(&c, &app).as_ref(),
            Some(&running),
            "{what}: the group still runs the release it ran"
        );
    }
    assert!(moved.is_empty(), "the operator moved it to {moved:?}");
    assert_eq!(node.app(&app).await.unwrap().bundle, running);
}

/// An upgrade judged against a bundle the app's group has since left is refused by the group
/// (`BundleMoved`), so a racing second upgrade never lands on code the record does not name:
/// here, the second of two upgrades judged on 1.1.0 arrives after the first moved it to 1.2.0.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_upgrade_judged_on_a_bundle_the_group_left_is_refused() {
    use calimero_node::node::CallRequest;
    use calimero_node::types::Caller;
    let c = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&c, 0, "alice").await;
    let org = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    for version in ["1.1.0", "1.2.0"] {
        let _ok = alice
            .packages()
            .upload(release(SEALED, version, true), Some(&org))
            .await
            .unwrap();
    }
    let app = create(&alice, "1.1.0", Some(&org)).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let judged = node.app(&app).await.unwrap().bundle;
    let _up = alice
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.2.0" })), Some(&mut pk))
        .await
        .unwrap();
    let moved = node.app(&app).await.unwrap().bundle;
    assert_ne!(judged, moved);

    let late = node
        .call(&CallRequest::host(
            app.clone(),
            "__upgrade",
            json!({ "bundle": judged, "from": judged }),
            Caller::System,
        ))
        .await
        .unwrap();
    assert_eq!(late.error["code"], "BundleMoved", "{:?}", late.error);
    assert_eq!(group_bundle(&c, &app), Some(moved.clone()));
    assert_eq!(node.app(&app).await.unwrap().bundle, moved);
}

/// Two upgrades of one app at once, as the operator may send them: whatever the order the
/// group and the log take them in, the group ends on the bundle the record names.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn concurrent_upgrades_leave_the_group_on_the_recorded_bundle() {
    use calimero_node::system::{Authority, Governance};
    let c = Cluster::start_with(3, |cfg| {
        cfg.governance = Governance {
            app_upgrades: Authority::Operator,
            ..Governance::default()
        };
    })
    .await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let ops = c.client(1);
    ops.use_api_key("ops-secret").await;
    for version in ["1.0.0", "1.1.0", "1.2.0"] {
        let _ok = ops
            .ops()
            .package_upload(release(SEALED, version, true))
            .await
            .unwrap();
    }
    let node = &c.nodes[0].as_ref().unwrap().node;
    for round in 0..3 {
        let app = create(&alice, "1.0.0", None).await;
        let (a, b) = (c.client(1), c.client(2));
        a.use_api_key("ops-secret").await;
        b.use_api_key("ops-secret").await;
        let path = format!("ops/apps/{app}/upgrade");
        let (_x, _y) = tokio::join!(
            harness::ops_signed(&a, &path, json!({ "version": "1.2.0" })),
            harness::ops_signed(&b, &path, json!({ "version": "1.1.0" })),
        );
        let record = node.app(&app).await.unwrap().bundle;
        let mut group = None;
        for _ in 0..50 {
            group = group_bundle(&c, &app);
            if group.as_ref() == Some(&record) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        assert_eq!(group, Some(record), "round {round}");
    }
}
