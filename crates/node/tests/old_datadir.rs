//! A data directory written before components existed opens on this node, and its
//! apps keep running on wasmer.
//!
//! `fixtures/datadir-a09547b2b.tar.gz` is one node's data directory as
//! release-v2-parity at `a09547b2b` wrote it (its own `calimero-node`, run in
//! process: one node on port 39517, storage key `[42; 32]`, a `kv-store` app with
//! `a = 1` and `b = 2`); `fixture.json` inside it names the app, its bundle and
//! its owner. Neither its directory record nor its group's state names an executor.

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_node::attest::Mode;
use calimero_node::node::{CallRequest, ViewRequest};
use calimero_node::types::{Caller, Executor};
use calimero_node::{run, Config};
use serde_json::{json, Value};

fn config(dir: &std::path::Path, port: u16) -> Config {
    let url = format!("http://127.0.0.1:{port}/");
    Config {
        node_id: 1,
        public_url: url.clone(),
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        data_dir: dir.to_owned(),
        peers: BTreeMap::from([(1, url)]),
        bootstrap: false,
        cluster: "test".into(),
        storage_key: [42; 32],
        attestation: Mode::Mock,
        allow_mock_peers: true,
        sealed_required: true,
        rp_id: "localhost".into(),
        origins: vec!["http://localhost:3000".into()],
        release: "test".into(),
        ops_token: Some("ops-secret".into()),
        ops_token_sha256: None,
        metrics_listen: None,
        replicas: 1,
        backup_dir: None,
        raft: calimero_log::Config {
            heartbeat_ms: 100,
            election_min_ms: 500,
            election_max_ms: 1000,
            snapshot_every: 200,
        },
        session_key_rotation_secs: 6 * 3600,
        read_wait: Duration::from_secs(2),
        tee_release: None,
        tee_profile: None,
        peer_measurements: calimero_client::Measurements::None,
        clock_offset_ms: 0,
        budget: None,
        registry: None,
        allow_unsigned_bundles: true,
        outbound: None,
        governance: Default::default(),
        apps_domain: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_directory_from_before_components_opens_and_its_apps_run_on_wasmer() {
    let tmp = tempfile::tempdir().unwrap();
    let archive = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/datadir-a09547b2b.tar.gz"),
    )
    .unwrap();
    tar::Archive::new(flate2::read::GzDecoder::new(&archive[..]))
        .unpack(tmp.path())
        .unwrap();
    let dir = tmp.path().join("node1");
    let fixture: Value =
        serde_json::from_slice(&std::fs::read(dir.join("fixture.json")).unwrap()).unwrap();
    let app = fixture["app"].as_str().unwrap().to_owned();
    let owner = fixture["owner"].as_str().unwrap().to_owned();
    let port = u16::try_from(fixture["port"].as_u64().unwrap()).unwrap();

    let r = run(config(&dir, port)).await.unwrap();
    let node = &r.node;
    let record = node.app(&app).await.unwrap();
    assert_eq!(record.bundle, fixture["bundle"].as_str().unwrap());
    assert_eq!(
        record.executor,
        Executor::Wasmer,
        "no recorded executor: wasmer"
    );
    let mut group = None;
    for _ in 0..200 {
        group = node.hosted(&app);
        if group
            .as_ref()
            .is_some_and(|g| g.status().is_leader && g.read_local(|m, _| m.state.initialized))
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let group = group.expect("the app group started");
    assert_eq!(
        group.read_local(|m, _| (m.state.executor, m.state.executor())),
        (None, Some(Executor::Wasmer)),
        "the old state names no executor and is applied as wasmer"
    );

    let caller = Caller::Account {
        account: owner.clone(),
    };
    let view = |key: &str| ViewRequest {
        app_id: app.clone(),
        method: "get".into(),
        args: json!({ "key": key }),
        caller: caller.clone(),
        strong: true,
        min_index: 0,
    };
    for (k, v) in fixture["values"].as_object().unwrap() {
        let a = node.view(&view(k)).await.unwrap();
        assert_eq!(a.result, *v, "{k}");
    }
    // It keeps running: a call writes, on wasmer, and reads back.
    let a = node
        .call(&CallRequest::new(
            app.clone(),
            "set".into(),
            json!({ "key": "c", "value": "3" }),
            caller.clone(),
        ))
        .await
        .unwrap();
    assert!(!a.conflict, "{a:?}");
    assert_eq!(node.view(&view("c")).await.unwrap().result, json!("3"));
    assert_eq!(
        group.read_local(|m, _| m.state.executor()),
        Some(Executor::Wasmer)
    );
    r.stop().await;
}
