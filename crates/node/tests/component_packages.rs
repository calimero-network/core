//! Rooms, a `calimero:app` component, installed as a signed `.mpk` the way
//! the node installs packages: uploaded (`POST /packages`) and from the
//! operator's registry (`POST /packages/install`), then created from its release.
//! A package whose WASM is a component runs on the component executor like a raw
//! component upload does.

mod common;

use std::collections::BTreeMap;

use calimero_client::{Client, Consistency};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::types::{Components, Executor};
use common::{mpk_parts, mpk_with, mpk_with_pipelines, signed_up, Cluster, PUBLISHER};
use serde_json::{json, Value};

/// A signed single-service `.mpk`, the shape `mero-sign` signs.
fn mpk(package: &str, version: &str, wasm: &[u8], key: &ed25519_dalek::SigningKey) -> Vec<u8> {
    mpk_with(package, version, wasm, None, key)
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

fn abi(component: &[u8]) -> Value {
    calimero_app_build::abi_of(component)
        .unwrap()
        .expect("an ABI")
}

/// `component` with its ABI changed by `edit`.
fn patched(component: &[u8], edit: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut abi = abi(component);
    edit(&mut abi);
    calimero_app_build::with_abi(component, &abi).unwrap()
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

async fn compute_cluster() -> (Cluster, Client, SoftAuthenticator) {
    let c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = false).await;
    let (owner, pk) = signed_up(&c, 0, "owner").await;
    (c, owner, pk)
}

async fn install(owner: &Client, version: &str, wasm: &[u8], compute: Option<&[u8]>) -> Value {
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    owner
        .upload_package(mpk_with("com.test.compute", version, wasm, compute, &key))
        .await
        .unwrap()
}

async fn refused(owner: &Client, wasm: &[u8], compute: &[u8]) -> calimero_client::Error {
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    owner
        .upload_package(mpk_with(
            "com.test.compute",
            "1.0.0",
            wasm,
            Some(compute),
            &key,
        ))
        .await
        .unwrap_err()
}

async fn create(owner: &Client, version: &str) -> String {
    let created = owner
        .post(
            "apps",
            json!({ "name": "compute", "package": "com.test.compute", "version": version }),
        )
        .await
        .unwrap();
    created["appId"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_release_with_both_components_records_both_and_its_app_runs_calls() {
    let (c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().expect("a compute component");
    install(&owner, "1.0.0", &both.app, Some(&job)).await;

    let (main_hash, job_hash) = (sha256_hex(&both.app), sha256_hex(&job));
    let node = &c.nodes[0].as_ref().unwrap().node;
    node.sys(|s| {
        let release = &s.packages["com.test.compute"].versions["1.0.0"];
        assert_eq!(release.services[""], main_hash);
        assert_eq!(release.compute[""], job_hash);
        let at = s.bundles[&job_hash].package.as_ref().expect("a release");
        assert!(at.compute, "{at:?}");
        assert!(!s.bundles[&main_hash].package.as_ref().unwrap().compute);
    });
    assert!(node.engine.get_blob(&job_hash).unwrap().is_some());

    let app = create(&owner, "1.0.0").await;
    let record = node.app(&app).await.unwrap();
    assert_eq!(record.bundle, main_hash);
    assert_eq!(record.compute_bundle.as_deref(), Some(job_hash.as_str()));
    let done = owner
        .view(&app, "done", json!({}), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(done.result, json!("0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_release_without_a_compute_artifact_installs_and_its_app_runs_calls() {
    let (c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    install(&owner, "1.0.0", &both.app, None).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    assert!(
        node.sys(|s| s.packages["com.test.compute"].versions["1.0.0"]
            .compute
            .is_empty())
    );

    let app = create(&owner, "1.0.0").await;
    assert_eq!(node.app(&app).await.unwrap().compute_bundle, None);
    let done = owner
        .view(&app, "done", json!({}), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(done.result, json!("0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_abi_that_disagrees_with_the_main_abi_is_refused() {
    let (_c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    let cheaper = patched(&job, |a| {
        a["methods"]["digest"]["compute"]["fuel"] = json!(1)
    });
    let e = refused(&owner, &both.app, &cheaper).await;
    assert_eq!(e.code(), Some("COMPUTE_ABI_MISMATCH"), "{e}");

    let extra = patched(&job, |a| {
        a["methods"]["other"] = a["methods"]["digest"].clone();
    });
    let e = refused(&owner, &both.app, &extra).await;
    assert_eq!(e.code(), Some("COMPUTE_ABI_MISMATCH"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_component_that_imports_state_is_refused() {
    let (_c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    // The app component imports `calimero:app/storage`, which no compute world has.
    let e = refused(&owner, &both.app, &both.app).await;
    assert_eq!(e.code(), Some("BAD_BUNDLE"), "{e}");
    assert!(e.to_string().contains("calimero:app/storage"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_component_must_carry_a_compute_world_abi() {
    let (_c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    let app_abi = abi(&both.app);
    let wrong_world = calimero_app_build::with_abi(&job, &app_abi).unwrap();
    let e = refused(&owner, &both.app, &wrong_world).await;
    assert_eq!(e.code(), Some("BAD_BUNDLE"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_component_cannot_be_a_services_main_component() {
    let (_c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let e = owner
        .upload_package(mpk("com.test.compute", "1.0.0", &job, &key))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_BUNDLE"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn the_running_releases_compute_hash_follows_an_upgrade() {
    let (c, owner, mut pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    install(&owner, "1.0.0", &both.app, Some(&job)).await;
    let main2 = patched(&both.app, |a| a["build"] = json!(2));
    let job2 = patched(&job, |a| a["build"] = json!(2));
    install(&owner, "1.0.1", &main2, Some(&job2)).await;
    install(
        &owner,
        "1.0.2",
        &patched(&both.app, |a| a["build"] = json!(3)),
        None,
    )
    .await;

    let app = create(&owner, "1.0.0").await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let running = || async { node.app(&app).await.unwrap().compute_bundle };
    // What the app's group runs, which a run pins: set with the bundle by the same entry.
    let in_group = || {
        node.hosted(&app)
            .unwrap()
            .read_local(|m, _| m.state.component.head.compute_bundle.clone())
    };
    assert_eq!(running().await, Some(sha256_hex(&job)));
    assert_eq!(in_group(), Some(sha256_hex(&job)));
    for (version, expect) in [("1.0.1", Some(sha256_hex(&job2))), ("1.0.2", None)] {
        let up = owner
            .upgrade_app(&app, json!({ "version": version }), &mut pk)
            .await
            .unwrap();
        assert_eq!(up["status"], "ok", "{up}");
        assert_eq!(up["compute"], json!(expect.is_some()), "{up}");
        assert_eq!(running().await, expect, "{version}");
        assert_eq!(in_group(), expect, "{version}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_app_created_or_upgraded_by_bundle_hash_takes_that_releases_compute_component() {
    let (c, owner, mut pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    let main2 = patched(&both.app, |a| a["build"] = json!(2));
    let job2 = patched(&job, |a| a["build"] = json!(2));
    let main3 = patched(&both.app, |a| a["build"] = json!(3));
    install(&owner, "1.0.0", &both.app, Some(&job)).await;
    install(&owner, "1.0.1", &main2, Some(&job2)).await;
    install(&owner, "1.0.2", &main3, None).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    let by_hash = |wasm: &[u8]| json!({ "name": "compute", "bundle": sha256_hex(wasm) });

    let created = owner.post("apps", by_hash(&both.app)).await.unwrap();
    assert_eq!(created["compute"], json!(true), "{created}");
    let app = created["appId"].as_str().unwrap().to_owned();
    let running = || async { node.app(&app).await.unwrap().compute_bundle };
    assert_eq!(running().await, Some(sha256_hex(&job)));

    let up = owner
        .upgrade_app(&app, json!({ "bundle": sha256_hex(&main2) }), &mut pk)
        .await
        .unwrap();
    assert_eq!(up["compute"], json!(true), "{up}");
    assert_eq!(running().await, Some(sha256_hex(&job2)));

    let up = owner
        .upgrade_app(&app, json!({ "bundle": sha256_hex(&main3) }), &mut pk)
        .await
        .unwrap();
    assert_eq!(up["compute"], json!(false), "{up}");
    assert_eq!(running().await, None);
    let plain = owner.post("apps", by_hash(&main3)).await.unwrap();
    assert_eq!(plain["compute"], json!(false), "{plain}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_bundle_hash_shared_by_releases_with_different_compute_is_ambiguous() {
    let (_c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    let job2 = patched(&job, |a| a["build"] = json!(2));
    install(&owner, "1.0.0", &both.app, Some(&job)).await;
    install(&owner, "1.0.1", &both.app, Some(&job2)).await;
    let e = owner
        .post(
            "apps",
            json!({ "name": "compute", "bundle": sha256_hex(&both.app) }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("AMBIGUOUS_COMPUTE"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_release_that_changes_only_compute_code_gives_its_app_its_own_compute_hash() {
    let (c, owner, mut pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    let job2 = patched(&job, |a| a["build"] = json!(2));
    install(&owner, "1.0.0", &both.app, Some(&job)).await;
    // The app component is byte-identical: only the compute component differs.
    install(&owner, "1.0.1", &both.app, Some(&job2)).await;
    let node = &c.nodes[0].as_ref().unwrap().node;

    let created = create(&owner, "1.0.1").await;
    assert_eq!(
        node.app(&created).await.unwrap().compute_bundle,
        Some(sha256_hex(&job2)),
        "created from the second release"
    );

    let app = create(&owner, "1.0.0").await;
    assert_eq!(
        node.app(&app).await.unwrap().compute_bundle,
        Some(sha256_hex(&job))
    );
    let up = owner
        .upgrade_app(&app, json!({ "version": "1.0.1" }), &mut pk)
        .await
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    assert_eq!(
        node.app(&app).await.unwrap().compute_bundle,
        Some(sha256_hex(&job2)),
        "upgraded to the second release"
    );
}

async fn install_compute_only(
    owner: &Client,
    version: &str,
    compute: &[u8],
) -> Result<Value, Box<calimero_client::Error>> {
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    owner
        .upload_package(mpk_parts(
            "com.test.job",
            version,
            None,
            Some(compute),
            &key,
        ))
        .await
        .map_err(Box::new)
}

async fn create_compute_only(owner: &Client, version: &str) -> String {
    let org = owner.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = owner
        .post(
            "apps",
            json!({ "name": "job", "package": "com.test.job", "version": version, "org": org }),
        )
        .await
        .unwrap();
    created["appId"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_only_package_installs() {
    let (c, owner, _pk) = compute_cluster().await;
    let job = common::components("compute").compute.clone().unwrap();
    install_compute_only(&owner, "1.0.0", &job).await.unwrap();

    let job_hash = sha256_hex(&job);
    let node = &c.nodes[0].as_ref().unwrap().node;
    node.sys(|s| {
        let release = &s.packages["com.test.job"].versions["1.0.0"];
        assert_eq!(release.components, Components::ComputeOnly);
        assert!(release.services.is_empty(), "{:?}", release.services);
        assert_eq!(release.compute[""], job_hash);
        assert!(s.bundles[&job_hash].package.as_ref().unwrap().compute);
    });

    let app = create_compute_only(&owner, "1.0.0").await;
    let record = node.app(&app).await.unwrap();
    assert_eq!(record.components, Components::ComputeOnly);
    assert_eq!(record.bundle, job_hash);
    assert_eq!(record.compute_bundle.as_deref(), Some(job_hash.as_str()));
    assert_eq!(node.bundle_wasm(&record.bundle).await.unwrap(), job);
}

/// An older node would drop `components` and apply a compute-only release as an app's,
/// so such a release is installed only once every node runs this release.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_only_package_waits_until_every_node_runs_this_release() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.allow_unsigned_bundles = false;
        cfg.release = if cfg.node_id == 3 { "0.9.0" } else { "1.0.0" }.into();
    })
    .await;
    let (owner, _pk) = signed_up(&c, 0, "owner").await;
    let job = common::components("compute").compute.clone().unwrap();
    let e = install_compute_only(&owner, "1.0.0", &job)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CLUSTER_NOT_UPGRADED"), "{e}");
    let node = &c.nodes[0].as_ref().unwrap().node;
    assert!(node.sys(|s| !s.packages.contains_key("com.test.job")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_only_package_with_no_compute_methods_is_refused() {
    let (_c, owner, _pk) = compute_cluster().await;
    let job = common::components("compute").compute.clone().unwrap();
    let none = patched(&job, |a| a["methods"] = json!({}));
    let e = install_compute_only(&owner, "1.0.0", &none)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NO_COMPUTE_METHODS"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_only_app_answers_no_app_component_to_calls_and_views() {
    let (c, owner, _pk) = compute_cluster().await;
    let job = common::components("compute").compute.clone().unwrap();
    install_compute_only(&owner, "1.0.0", &job).await.unwrap();
    let app = create_compute_only(&owner, "1.0.0").await;
    // Creating answers once the group's empty init committed, so a pipeline can start at once.
    let initialized: Vec<_> = c
        .nodes
        .iter()
        .flatten()
        .filter_map(|r| r.node.hosted(&app))
        .filter(|g| g.read_local(|m, _| m.state.initialized))
        .collect();
    let group = initialized.first().expect("a replica applied the init");
    group.read_local(|m, _| {
        assert_eq!(
            m.state.component.head.compute_bundle,
            Some(sha256_hex(&job))
        );
        assert!(m.state.init_error.is_none());
    });

    let e = owner
        .call(&app, "digest", json!({}), None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NO_APP_COMPONENT"), "{e}");
    let e = owner
        .view(&app, "done", json!({}), Consistency::Strong)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NO_APP_COMPONENT"), "{e}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_app_package_installs_exactly_as_before() {
    let (c, owner, _pk) = compute_cluster().await;
    let both = common::components("compute");
    install(&owner, "1.0.0", &both.app, both.compute.as_deref()).await;
    let node = &c.nodes[0].as_ref().unwrap().node;
    node.sys(|s| {
        let release = &s.packages["com.test.compute"].versions["1.0.0"];
        assert_eq!(release.components, Components::App);
        assert_eq!(release.services[""], sha256_hex(&both.app));
        // A record of an app release reads and writes as before: no new field.
        let stored = serde_json::to_value(release).unwrap();
        assert!(stored.get("components").is_none());
        assert!(stored.get("pipelines").is_none());
    });
    let app = create(&owner, "1.0.0").await;
    let record = node.app(&app).await.unwrap();
    assert_eq!(record.components, Components::App);
    assert!(serde_json::to_value(&record)
        .unwrap()
        .get("components")
        .is_none());
    let done = owner
        .view(&app, "done", json!({}), Consistency::Strong)
        .await
        .unwrap();
    assert_eq!(done.result, json!("0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn switching_components_on_upgrade_is_refused() {
    let (c, owner, mut pk) = compute_cluster().await;
    let both = common::components("compute");
    let job = both.compute.clone().unwrap();
    let job2 = patched(&job, |a| a["build"] = json!(2));
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let publish = |version: &str, wasm: Option<&[u8]>, compute: &[u8]| {
        owner.upload_package(mpk_parts(
            "com.test.job",
            version,
            wasm,
            Some(compute),
            &key,
        ))
    };
    let _ = publish("1.0.0", None, &job).await.unwrap();
    let _ = publish("1.1.0", None, &job2).await.unwrap();
    let _ = publish("2.0.0", Some(&both.app), &job).await.unwrap();
    let node = &c.nodes[0].as_ref().unwrap().node;

    // Compute-only to compute-only moves the directory record.
    let app = create_compute_only(&owner, "1.0.0").await;
    let up = owner
        .upgrade_app(&app, json!({ "version": "1.1.0" }), &mut pk)
        .await
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    let record = node.app(&app).await.unwrap();
    assert_eq!(record.bundle, sha256_hex(&job2));
    assert_eq!(record.compute_bundle, Some(sha256_hex(&job2)));

    // Compute-only to a release with an app component is refused.
    let e = owner
        .upgrade_app(&app, json!({ "version": "2.0.0" }), &mut pk)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("COMPONENTS_CHANGED"), "{e}");
    assert_eq!(node.app(&app).await.unwrap().bundle, sha256_hex(&job2));

    // And the other way.
    let with_app = owner
        .post(
            "apps",
            json!({ "name": "both", "package": "com.test.job", "version": "2.0.0" }),
        )
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    let e = owner
        .upgrade_app(&with_app, json!({ "version": "1.0.0" }), &mut pk)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("COMPONENTS_CHANGED"), "{e}");
}

const PIPELINE: &str = "apiVersion: compute.calimero.network/v1
kind: Pipeline
name: p1
inputs:
  docs: { kind: files }
steps:
  first: { method: digest, inputs: [inputs.docs] }
  second: { method: burn, needs: [first] }
";

async fn install_with_pipelines(
    owner: &Client,
    pipelines: &[(&str, &str)],
) -> Result<Value, Box<calimero_client::Error>> {
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let job = common::components("compute").compute.clone().unwrap();
    owner
        .upload_package(mpk_with_pipelines(
            "com.test.job",
            "1.0.0",
            None,
            Some(&job),
            pipelines,
            &key,
        ))
        .await
        .map_err(Box::new)
}

fn bad_pipeline(e: &calimero_client::Error) -> Vec<Value> {
    assert_eq!(e.code(), Some("BAD_PIPELINE"), "{e}");
    e.details().unwrap().as_array().unwrap().clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_compute_only_package_with_a_pipeline_installs_and_lists_it() {
    let (c, owner, _pk) = compute_cluster().await;
    install_with_pipelines(&owner, &[("pipelines/p1.yaml", PIPELINE)])
        .await
        .unwrap();

    let canonical =
        calimero_pipeline::canonical_json(&calimero_pipeline::compile(PIPELINE).unwrap());
    let listed = owner
        .get("packages/com.test.job/1.0.0/pipelines")
        .await
        .unwrap();
    let p1 = &listed["p1"];
    assert_eq!(p1["hash"], sha256_hex(&canonical));
    assert_eq!(p1["worstCaseFuel"], "20001000000");
    assert_eq!(p1["pipeline"]["name"], "p1");
    assert_eq!(p1["pipeline"]["steps"]["second"]["needs"], json!(["first"]));

    let node = &c.nodes[0].as_ref().unwrap().node;
    assert_eq!(
        node.pipeline_json(&sha256_hex(&canonical)).await.unwrap(),
        canonical
    );
    // A node that lost the part recompiles it from the release.
    node.engine.delete_blob(&sha256_hex(&canonical)).unwrap();
    assert_eq!(
        node.pipeline_json(&sha256_hex(&canonical)).await.unwrap(),
        canonical
    );
    let none = owner.get("packages/com.test.job/9.9.9/pipelines").await;
    assert_eq!(none.unwrap_err().code(), Some("NOT_FOUND"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_pipeline_naming_an_unknown_method_is_refused_with_its_path() {
    let (_c, owner, _pk) = compute_cluster().await;
    let bad = PIPELINE.replace("method: burn", "method: ghost");
    let e = install_with_pipelines(&owner, &[("pipelines/p1.yaml", &bad)])
        .await
        .unwrap_err();
    let details = bad_pipeline(&e);
    assert_eq!(details.len(), 1, "{details:?}");
    assert_eq!(details[0]["file"], "pipelines/p1.yaml");
    assert_eq!(details[0]["path"], "steps.second.method");
    assert_eq!(details[0]["code"], "UNKNOWN_METHOD");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_pipeline_on_another_module_is_refused() {
    let (_c, owner, _pk) = compute_cluster().await;
    let other = PIPELINE.replace("name: p1\n", "name: p1\nmodule: calimero/pdf-extract@1\n");
    let e = install_with_pipelines(&owner, &[("pipelines/p1.yaml", &other)])
        .await
        .unwrap_err();
    assert!(bad_pipeline(&e)
        .iter()
        .all(|d| d["code"] == "UNKNOWN_MODULE"));
    let step = PIPELINE.replace("method: burn", "module: other, method: burn");
    let e = install_with_pipelines(&owner, &[("pipelines/p1.yaml", &step)])
        .await
        .unwrap_err();
    assert_eq!(bad_pipeline(&e)[0]["path"], "steps.second.module");
    // `self` is the package's own component.
    let own = PIPELINE.replace("name: p1\n", "name: p1\nmodule: self\n");
    install_with_pipelines(&owner, &[("pipelines/p1.yaml", &own)])
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn pipelines_are_refused_when_malformed_duplicated_or_without_a_compute_component() {
    let (_c, owner, _pk) = compute_cluster().await;
    let broken =
        "apiVersion: compute.calimero.network/v1\nkind: Pipeline\nname: Bad Name\nsteps: {}\n";
    let e = install_with_pipelines(&owner, &[("pipelines/bad.yaml", broken)])
        .await
        .unwrap_err();
    assert!(!bad_pipeline(&e).is_empty());

    let e = install_with_pipelines(
        &owner,
        &[
            ("pipelines/a.yaml", PIPELINE),
            ("pipelines/b.yaml", PIPELINE),
        ],
    )
    .await
    .unwrap_err();
    assert_eq!(bad_pipeline(&e)[0]["code"], "DUPLICATE_NAME");

    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let both = common::components("compute");
    let e = owner
        .upload_package(mpk_with_pipelines(
            "com.test.job",
            "1.0.0",
            Some(&both.app),
            None,
            &[("pipelines/p1.yaml", PIPELINE)],
            &key,
        ))
        .await
        .unwrap_err();
    assert_eq!(bad_pipeline(&e)[0]["code"], "NO_COMPUTE");
}

/// An older node drops `pipelines` and would run a release's pipelines as none.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_package_with_pipelines_waits_until_every_node_runs_this_release() {
    let c = Cluster::start_with(3, |cfg| {
        cfg.allow_unsigned_bundles = false;
        cfg.release = if cfg.node_id == 3 { "0.9.0" } else { "1.0.0" }.into();
    })
    .await;
    let (owner, _pk) = signed_up(&c, 0, "owner").await;
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let both = common::components("compute");
    let with_app = |pipelines: &[(&str, &str)]| {
        mpk_with_pipelines(
            "com.test.compute",
            "1.0.0",
            Some(&both.app),
            both.compute.as_deref(),
            pipelines,
            &key,
        )
    };
    let e = owner
        .upload_package(with_app(&[("pipelines/p1.yaml", PIPELINE)]))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CLUSTER_NOT_UPGRADED"), "{e}");
    // A release without pipelines installs as before.
    owner.upload_package(with_app(&[])).await.unwrap();
}

const MODULE_PACKAGE: &str = "com.test.module";

/// The fixture's compute component with its own build and a lower fuel cap on `digest` and
/// `check`, so it is another release with other caps.
fn module_component(build: u64) -> Vec<u8> {
    let job = common::components("compute").compute.clone().unwrap();
    patched(&job, |a| {
        a["build"] = json!(build);
        for method in ["digest", "check"] {
            a["methods"][method]["compute"]["fuel"] = json!(1_000_000_000_u64);
        }
    })
}

async fn install_module(owner: &Client, version: &str, build: u64) -> Value {
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    owner
        .upload_package(mpk_parts(
            MODULE_PACKAGE,
            version,
            None,
            Some(&module_component(build)),
            &key,
        ))
        .await
        .unwrap()
}

/// `PIPELINE` with its first step in `module`, and `extra` after the step's method.
fn module_pipeline(module: &str, extra: &str) -> String {
    PIPELINE.replace(
        "first: { method: digest",
        &format!("first: {{ module: {module}, {extra}method: digest"),
    )
}

async fn install_user(
    owner: &Client,
    version: &str,
    source: &str,
) -> Result<Value, Box<calimero_client::Error>> {
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let job = common::components("compute").compute.clone().unwrap();
    owner
        .upload_package(mpk_with_pipelines(
            "com.test.job",
            version,
            None,
            Some(&job),
            &[("pipelines/p1.yaml", source)],
            &key,
        ))
        .await
        .map_err(Box::new)
}

async fn pins(owner: &Client, version: &str) -> Value {
    let listed = owner
        .get(&format!("packages/com.test.job/{version}/pipelines"))
        .await
        .unwrap();
    listed["p1"]["modules"].clone()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_pipeline_step_names_a_module_from_another_installed_package() {
    let (c, owner, _pk) = compute_cluster().await;
    install_module(&owner, "1.0.0", 1).await;
    let reference = format!("{MODULE_PACKAGE}@1.0.0");
    install_user(&owner, "1.0.0", &module_pipeline(&reference, ""))
        .await
        .unwrap();

    let hash = sha256_hex(&module_component(1));
    assert_eq!(
        pins(&owner, "1.0.0").await,
        json!({ reference.clone(): { "package": MODULE_PACKAGE, "version": "1.0.0", "compute": hash } })
    );
    let node = &c.nodes[0].as_ref().unwrap().node;
    node.sys(|s| {
        let p1 = &s.packages["com.test.job"].versions["1.0.0"].pipelines["p1"];
        assert_eq!(p1.modules[&reference].compute, hash);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_module_the_installer_cannot_see_is_unknown() {
    let (c, owner, _pk) = compute_cluster().await;
    let unknown = |e: &calimero_client::Error| {
        let details = bad_pipeline(e);
        assert_eq!(details.len(), 1, "{details:?}");
        assert_eq!(details[0]["path"], "steps.first.module");
        assert_eq!(details[0]["code"], "UNKNOWN_MODULE");
    };
    let reference = format!("{MODULE_PACKAGE}@1.0.0");

    // Not installed at all, or not at that version.
    let e = install_user(&owner, "1.0.0", &module_pipeline(&reference, ""))
        .await
        .unwrap_err();
    unknown(&e);
    install_module(&owner, "1.0.0", 1).await;
    let wrong = format!("{MODULE_PACKAGE}@9.9.9");
    let e = install_user(&owner, "1.0.0", &module_pipeline(&wrong, ""))
        .await
        .unwrap_err();
    unknown(&e);

    // Installed by an account the other shares nothing with.
    let (other, _pk2) = signed_up(&c, 1, "other").await;
    let e = install_user(&other, "1.0.0", &module_pipeline(&reference, ""))
        .await
        .unwrap_err();
    unknown(&e);

    // A package with no compute component is no module.
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let app = common::components("compute");
    let _installed = owner
        .upload_package(mpk_with(
            "com.test.noncompute",
            "1.0.0",
            &app.app,
            None,
            &key,
        ))
        .await
        .unwrap();
    let e = install_user(
        &owner,
        "1.0.0",
        &module_pipeline("com.test.noncompute@1.0.0", ""),
    )
    .await
    .unwrap_err();
    unknown(&e);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_limit_above_the_modules_caps_is_refused() {
    let (_c, owner, _pk) = compute_cluster().await;
    install_module(&owner, "1.0.0", 1).await;
    let reference = format!("{MODULE_PACKAGE}@1.0.0");
    // 2e9 is within the package's own cap for `digest` (2e10) and over the module's (1e9).
    let over = module_pipeline(&reference, "limits: { fuel: 2000000000 }, ");
    let e = install_user(&owner, "1.0.0", &over).await.unwrap_err();
    let details = bad_pipeline(&e);
    assert_eq!(details.len(), 1, "{details:?}");
    assert_eq!(details[0]["path"], "steps.first.limits.fuel");
    assert_eq!(details[0]["code"], "LIMIT_EXCEEDS_CAP");
    let at = module_pipeline(&reference, "limits: { fuel: 1000000000 }, ");
    install_user(&owner, "1.0.0", &at).await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn uninstalling_a_pinned_module_is_refused_until_its_user_is_gone() {
    let (_c, owner, _pk) = compute_cluster().await;
    install_module(&owner, "1.0.0", 1).await;
    let reference = format!("{MODULE_PACKAGE}@1.0.0");
    install_user(&owner, "1.0.0", &module_pipeline(&reference, ""))
        .await
        .unwrap();

    let e = owner
        .uninstall_package(MODULE_PACKAGE, Some("1.0.0"))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("MODULE_IN_USE"), "{e}");
    let e = owner
        .uninstall_package(MODULE_PACKAGE, None)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("MODULE_IN_USE"), "{e}");

    let _gone = owner.uninstall_package("com.test.job", None).await.unwrap();
    let _gone = owner
        .uninstall_package(MODULE_PACKAGE, Some("1.0.0"))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_newer_module_release_does_not_move_an_installed_pipeline() {
    let (_c, owner, _pk) = compute_cluster().await;
    install_module(&owner, "1.0.0", 1).await;
    // No version: the newest release the installer sees.
    let source = module_pipeline(MODULE_PACKAGE, "");
    install_user(&owner, "1.0.0", &source).await.unwrap();
    let v1 = sha256_hex(&module_component(1));
    assert_eq!(pins(&owner, "1.0.0").await[MODULE_PACKAGE]["compute"], v1);

    install_module(&owner, "2.0.0", 2).await;
    assert_eq!(
        pins(&owner, "1.0.0").await[MODULE_PACKAGE]["compute"],
        v1,
        "an installed pipeline keeps the module it was installed against"
    );
    // Releasing the pipeline's own package again takes the newer module.
    install_user(&owner, "1.1.0", &source).await.unwrap();
    let pinned = pins(&owner, "1.1.0").await;
    assert_eq!(pinned[MODULE_PACKAGE]["version"], "2.0.0");
    assert_eq!(
        pinned[MODULE_PACKAGE]["compute"],
        sha256_hex(&module_component(2))
    );
}
