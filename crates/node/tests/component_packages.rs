//! Rooms, a `calimero:app` component, installed as a signed `.mpk` the way
//! the node installs packages: uploaded (`POST /packages`) and from the
//! operator's registry (`POST /packages/install`), then created from its release.
//! A package whose WASM is a component runs on the component executor like a raw
//! component upload does.

mod common;

use std::collections::BTreeMap;

use calimero_client::{Client, Consistency};
use calimero_node::types::Executor;
use common::{signed_up, Cluster};
use serde_json::{json, Value};

/// A signed single-service `.mpk`, the shape `cargo mero bundle` produces.
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

/// A registry serving `{base}/artifacts/{package}/{version}/{file}`.
async fn serve_registry(files: BTreeMap<String, Vec<u8>>) -> String {
    use axum::extract::Path;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;
    let files = std::sync::Arc::new(files);
    let app = axum::Router::new().route(
        "/artifacts/{package}/{version}/{file}",
        axum::routing::get(
            move |Path((package, version, _file)): Path<(String, String, String)>| {
                let files = std::sync::Arc::clone(&files);
                async move {
                    match files.get(&format!("{package}@{version}")) {
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

/// An org of `owner`'s, with Rooms created in it from `package@version`; Rooms then
/// takes a room (an admin's call), refuses the same room again, and answers a view.
async fn run_rooms_from(c: &Cluster, owner: &Client, package: &str, version: &str) {
    let org = owner.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = owner
        .post(
            "apps",
            json!({ "name": "rooms", "package": package, "version": version, "org": org }),
        )
        .await
        .unwrap();
    let app = created["appId"].as_str().unwrap().to_owned();
    let record = c.nodes[0].as_ref().unwrap().node.app(&app).await.unwrap();
    assert_eq!(record.executor, Executor::Component, "a component package");
    let added = owner
        .call(
            &app,
            "addRoom",
            json!({ "id": "r1", "name": "Blue", "capacity": 4 }),
            None,
        )
        .await
        .unwrap();
    assert!(added.receipt.is_some(), "the answer carries a receipt");
    let e = owner
        .call(
            &app,
            "addRoom",
            json!({ "id": "r1", "name": "Blue", "capacity": 4 }),
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_ERROR"), "{e}");
    assert_eq!(e.details().unwrap()["code"], "Duplicate", "{e}");
    let mine = owner
        .view(&app, "myBookings", json!({}), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(mine.result, Value::Array(Vec::new()));
    // Every replica runs it on its component executor.
    for i in 0..3 {
        let node = &c.nodes[i].as_ref().unwrap().node;
        let mut executor = None;
        for _ in 0..100 {
            executor = node
                .hosted(&app)
                .and_then(|g| g.read_local(|m, _| m.state.executor()));
            if executor.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(executor, Some(Executor::Component), "node {i}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn rooms_installs_and_runs_as_an_uploaded_signed_package() {
    let c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = false).await;
    let (owner, _pk) = signed_up(&c, 0, "owner").await;
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let rooms = common::rooms();
    // Production nodes take no raw WASM, component or not.
    let e = owner.upload_bundle(rooms.clone()).await.unwrap_err();
    assert!(e.to_string().starts_with("403"), "{e}");
    let installed = owner
        .upload_package(mpk("com.test.rooms", "1.0.0", &rooms, &publisher))
        .await
        .unwrap();
    assert_eq!(installed["version"], json!("1.0.0"));
    run_rooms_from(&c, &owner, "com.test.rooms", "1.0.0").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn rooms_installs_and_runs_from_the_registry() {
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let rooms = common::rooms();
    let url = serve_registry(BTreeMap::from([(
        "com.test.rooms@1.0.0".to_owned(),
        mpk("com.test.rooms", "1.0.0", &rooms, &publisher),
    )]))
    .await;
    let c = Cluster::start_with(3, |cfg| {
        cfg.registry = Some(url.clone());
        cfg.allow_unsigned_bundles = false;
    })
    .await;
    let (owner, _pk) = signed_up(&c, 0, "owner").await;
    let installed = owner
        .install_package("com.test.rooms", "1.0.0")
        .await
        .unwrap();
    assert_eq!(installed["version"], json!("1.0.0"));
    run_rooms_from(&c, &owner, "com.test.rooms", "1.0.0").await;
}
