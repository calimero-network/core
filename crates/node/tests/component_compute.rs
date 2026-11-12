//! Starting and cancelling compute runs from app code on a cluster, through the `compute`
//! fixture (`apps/v2-fixtures/compute`). No job node is registered, so a run stays queued.

mod common;
use common::UploadBytes as _;

use calimero_client::Client;
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::component::{RunRecord, RunState};
use calimero_node::Node;
use common::{mpk_with, signed_up, typed, wire, Cluster, PUBLISHER};
use serde_json::json;
use sha2::Digest;

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(sha2::Sha256::digest(bytes))
}

/// The app component of the fixture and the compute components of two releases of it.
struct Releases {
    app: Vec<u8>,
    compute: [Vec<u8>; 2],
}

fn releases() -> Releases {
    let both = common::components("compute");
    let first = both.compute.clone().expect("a compute component");
    let mut abi = calimero_app_build::abi_of(&first).unwrap().expect("an ABI");
    abi["build"] = json!(2);
    let second = calimero_app_build::with_abi(&first, &abi).unwrap();
    Releases {
        app: both.app,
        compute: [first, second],
    }
}

/// A cluster that takes signed packages, with the first release (`1.0.0`) of the fixture installed.
async fn cluster_with_releases(r: &Releases) -> (Cluster, Client, SoftAuthenticator) {
    let c = Cluster::start_with(3, |cfg| cfg.allow_unsigned_bundles = false).await;
    let (owner, pk) = signed_up(&c, 0, "owner").await;
    install(&owner, "1.0.0", &r.app, &r.compute[0]).await;
    (c, owner, pk)
}

async fn install(owner: &Client, version: &str, app: &[u8], compute: &[u8]) {
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let _installed = owner
        .packages()
        .upload(mpk_with(
            "com.test.compute",
            version,
            app,
            Some(compute),
            &key,
        ))
        .await
        .unwrap();
}

async fn create(owner: &Client, version: &str) -> String {
    let created = owner
        .apps()
        .create(typed(
            json!({ "name": "compute", "package": "com.test.compute", "version": version }),
        ))
        .await
        .map(wire)
        .unwrap();
    created["appId"].as_str().unwrap().to_owned()
}

fn leader_record(c: &Cluster, app: &str, run: &str) -> (RunRecord, Vec<String>) {
    let leader = c.app_leader(app).unwrap();
    let node: &Node = &c.nodes[leader].as_ref().unwrap().node;
    let g = node.hosted(app).unwrap();
    g.read_local(|m, _| (m.run_record(run).unwrap(), m.active_runs()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_call_starts_a_queued_run_and_an_identical_call_answers_the_same_one() {
    let r = releases();
    let (c, alice, _pk) = cluster_with_releases(&r).await;
    let app = create(&alice, "1.0.0").await;
    // A plain blob is not an input: only the app's sealed files are.
    let plain = alice
        .apps()
        .blob_upload(&app, vec![7u8; 5000], None)
        .await
        .map(wire)
        .unwrap();
    let refused = alice
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [plain["id"]] })),
            None,
            None,
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    let up = alice.upload_bytes(&app, vec![7u8; 5000]).await.unwrap();
    let file = up["id"].as_str().unwrap().to_owned();

    let first = alice
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [file] })),
            None,
            None,
        )
        .await
        .unwrap();
    let run = first.result.as_str().unwrap().to_owned();
    let again = alice
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [file] })),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(again.result, json!(run), "the same job is the same run");

    let (record, active) = leader_record(&c, &app, &run);
    assert_eq!(record.state, RunState::Queued);
    assert_eq!(record.method, "digest");
    assert_eq!(record.inputs.len(), 1);
    assert_eq!(
        (record.inputs[0].id.as_str(), record.inputs[0].size),
        (file.as_str(), 5000)
    );
    assert_eq!(record.result_method, "onDone");
    assert!(!record.owner.is_empty());
    assert_eq!(active, std::slice::from_ref(&run));

    // A follower has applied the same record.
    let follower = (0..3).find(|i| Some(*i) != c.app_leader(&app)).unwrap();
    let mut seen = None;
    for _ in 0..50 {
        let node: &Node = &c.nodes[follower].as_ref().unwrap().node;
        seen = node
            .hosted(&app)
            .and_then(|g| g.read_local(|m, _| m.run_record(&run)));
        if seen.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert_eq!(seen.as_ref(), Some(&record), "the follower's record");

    // Other inputs are another run, and a file that is not the app's is refused.
    let other = alice
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [] })),
            None,
            None,
        )
        .await
        .unwrap();
    assert_ne!(other.result, json!(run));
    let missing = alice
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": ["00".repeat(32)] })),
            None,
            None,
        )
        .await;
    assert!(missing.is_err(), "{missing:?}");

    // A cancel is recorded on the active run; nothing grants it, so the leader may end it cancelled.
    let _ok = alice
        .apps()
        .call(&app, "cancel", Some(json!({ "run": run })), None, None)
        .await
        .unwrap();
    let (record, _) = leader_record(&c, &app, &run);
    assert!(record.cancel_requested);
    assert!(
        matches!(record.state, RunState::Queued | RunState::Cancelled),
        "{:?}",
        record.state
    );
    let unknown = alice
        .apps()
        .call(&app, "cancel", Some(json!({ "run": "nobody" })), None, None)
        .await;
    assert!(unknown.is_err(), "{unknown:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_start_on_an_app_without_a_compute_component_is_refused_and_records_nothing() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let bundle = alice
        .packages()
        .bundle_put(common::component("compute"))
        .await
        .map(|b| b.hash)
        .unwrap();
    let app = alice
        .apps()
        .create(typed(json!({ "name": "compute", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .unwrap();
    let up = alice.upload_bytes(&app, vec![7u8; 5000]).await.unwrap();

    let refused = alice
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [up["id"]] })),
            None,
            None,
        )
        .await;
    let error = format!("{refused:?}");
    assert!(error.contains("no compute component"), "{error}");

    let leader = c.app_leader(&app).unwrap();
    let node: &Node = &c.nodes[leader].as_ref().unwrap().node;
    let active = node
        .hosted(&app)
        .unwrap()
        .read_local(|m, _| m.active_runs());
    assert!(active.is_empty(), "{active:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_run_keeps_the_compute_component_it_started_on_through_an_upgrade() {
    let r = releases();
    let (c, owner, mut pk) = cluster_with_releases(&r).await;
    install(&owner, "1.0.1", &r.app, &r.compute[1]).await;
    let app = create(&owner, "1.0.0").await;

    let before = owner
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [] })),
            None,
            None,
        )
        .await
        .unwrap();
    let up = owner
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.0.1" })), Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], "ok", "{up}");
    let file = owner.upload_bytes(&app, vec![7u8; 5000]).await.unwrap();
    let after = owner
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [file["id"]] })),
            None,
            None,
        )
        .await
        .unwrap();

    let first = sha256_hex(&r.compute[0]);
    let second = sha256_hex(&r.compute[1]);
    let compute_of = |run: &serde_json::Value| {
        leader_record(&c, &app, run.as_str().unwrap())
            .0
            .compute_bundle
    };
    assert_eq!(compute_of(&before.result), Some(first));
    assert_eq!(compute_of(&after.result), Some(second));
}

/// `component` with its result method `onDone` renamed `onDone2` in its ABI.
fn renamed_result(component: &[u8]) -> Vec<u8> {
    let mut abi = calimero_app_build::abi_of(component)
        .unwrap()
        .expect("an ABI");
    let methods = abi["methods"].as_object_mut().unwrap();
    // The compute component's ABI lists only the compute methods.
    if let Some(on_done) = methods.remove("onDone") {
        let _prev = methods.insert("onDone2".into(), on_done);
    }
    for method in abi["methods"].as_object_mut().unwrap().values_mut() {
        if method["compute"]["result"] == "onDone" {
            method["compute"]["result"] = json!("onDone2");
        }
    }
    calimero_app_build::with_abi(component, &abi).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_upgrade_that_drops_the_result_method_of_an_active_run_is_refused() {
    let r = releases();
    let (_c, owner, mut pk) = cluster_with_releases(&r).await;
    install(
        &owner,
        "1.0.1",
        &renamed_result(&r.app),
        &renamed_result(&r.compute[0]),
    )
    .await;
    let idle = create(&owner, "1.0.0").await;
    let up = owner
        .apps()
        .upgrade(&idle, typed(json!({ "version": "1.0.1" })), Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(up["status"], "ok", "no run needs it: {up}");

    let app = create(&owner, "1.0.0").await;
    let _run = owner
        .apps()
        .call(
            &app,
            "startDigest",
            Some(json!({ "inputs": [] })),
            None,
            None,
        )
        .await
        .unwrap();
    let e = owner
        .apps()
        .upgrade(&app, typed(json!({ "version": "1.0.1" })), Some(&mut pk))
        .await
        .unwrap_err();
    assert!(
        format!("{:?}", e.details()).contains("RunNeedsResultMethod"),
        "{e}"
    );
}
