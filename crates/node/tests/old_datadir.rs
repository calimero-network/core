//! A data directory written before components existed opens on this node, which has
//! no wasmer: its SDK 0.12 app is still listed and can be deleted, and every call,
//! view and upgrade of it is `410 UNSUPPORTED_ENGINE`.
//!
//! `fixtures/datadir-a09547b2b.tar.gz` is one node's data directory as
//! the node at `a09547b2b` wrote it (its own `calimero-node`, run in
//! process: one node on port 39517, storage key `[42; 32]`, a `kv-store` app with
//! `a = 1` and `b = 2`); `fixture.json` inside it names the app, its bundle and
//! its owner. Neither its directory record nor its group's state names an executor.

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_node::attest::Mode;
use calimero_node::node::{CallRequest, ViewRequest};
use calimero_node::system::SystemCmd;
use calimero_node::types::{Caller, Executor};
use calimero_node::{run, Config};
use serde_json::{json, Value};

/// A port nothing listens on now.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

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
        recovery_dir: None,
        recover_from: None,
        restore_from: None,
        raft: calimero_log::Config {
            heartbeat_ms: 100,
            election_min_ms: 500,
            election_max_ms: 1000,
            snapshot_every: 200,
        },
        read_wait: Duration::from_secs(2),
        refresh_rotate_after: calimero_node::api::REFRESH_ROTATE_AFTER,
        tee_release: None,
        tee_profile: None,
        peer_measurements: calimero_client::Measurements::None,
        clock_offset_ms: 0,
        kms_keys: Default::default(),
        rollback: None,
        accept_rollback: false,
        budget: None,
        registry: None,
        allow_unsigned_bundles: true,
        allow_package_uploads: true,
        outbound: None,
        governance: Default::default(),
        apps_domain: None,
        operator_key: None,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_data_directory_from_before_components_opens_and_its_sdk_0_12_app_runs_nothing() {
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
    // The fixture was written on port 39517 (`fixture.json`'s `port`), which its store
    // records, but a single node never dials itself. A free port keeps this test from
    // failing when a test running in parallel holds that one.
    let port = free_port();

    let r = run(config(&dir, port)).await.unwrap();
    let node = &r.node;
    // Listed, as it was.
    let record = node.app(&app).await.unwrap();
    assert_eq!(record.bundle, fixture["bundle"].as_str().unwrap());
    assert_eq!(
        record.executor,
        Executor::Wasmer,
        "no recorded executor: wasmer"
    );
    assert!(node.sys(|s| s.apps.contains_key(&app)));
    // Its owner signed up before passkey and session times, and activity,
    // were kept: the account and its sessions read with them unknown, never
    // made up, and replaying the old log's sign-up and step-ups adds no
    // activity.
    let (times, activity, opened) = node.sys(|s| {
        (
            s.accounts[&owner].passkey_times.clone(),
            s.activity.get(&owner).cloned().unwrap_or_default(),
            s.refresh
                .values()
                .filter(|r| r.account == owner)
                .map(|r| r.opened_at)
                .collect::<Vec<_>>(),
        )
    });
    assert!(times.is_empty(), "{times:?}");
    assert!(activity.is_empty(), "{activity:?}");
    assert!(opened.iter().all(|t| *t == 0), "{opened:?}");
    // Its group opens on the state the old node left.
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
        "the old state names no executor and is an SDK 0.12 app's"
    );

    // It runs nothing: a view, a call and an upgrade are each `410 UNSUPPORTED_ENGINE`.
    let caller = Caller::Account {
        account: owner.clone(),
    };
    let gone = |e: calimero_node::error::ApiError| {
        assert_eq!(
            (e.status, e.code.as_str()),
            (410, "UNSUPPORTED_ENGINE"),
            "{e:?}"
        );
        assert!(e.message.contains("calimero-sdk"), "{e:?}");
    };
    for k in fixture["values"].as_object().unwrap().keys() {
        gone(
            node.view(&ViewRequest {
                app_id: app.clone(),
                method: "get".into(),
                args: json!({ "key": k }),
                caller: caller.clone(),
                strong: true,
                min_index: 0,
            })
            .await
            .unwrap_err(),
        );
    }
    gone(
        node.call(&CallRequest::new(
            app.clone(),
            "set".into(),
            json!({ "key": "c", "value": "3" }),
            caller.clone(),
        ))
        .await
        .unwrap_err(),
    );
    gone(
        node.call(&CallRequest::host(
            app.clone(),
            "__upgrade",
            json!({ "bundle": record.bundle }),
            caller.clone(),
        ))
        .await
        .unwrap_err(),
    );
    // A move of it, new in this release, is judged on the old state like any
    // command: without its owner's passkey it is refused, and the record
    // stays as the old node wrote it (personal, its owner's payer).
    let refused = node
        .sys_write(&SystemCmd::MoveApp {
            app_id: app.clone(),
            to_org: "nowhere".into(),
            actor: owner.clone(),
            consent: Value::Null,
            bring_members: false,
            nonce: String::new(),
            all_apps_rows: true,
            accounts_in_app_team: true,
        })
        .await
        .unwrap_err();
    assert_eq!(refused.status, 400, "{refused:?}");
    let kept = node.app(&app).await.unwrap();
    assert_eq!(kept.org, None);
    assert_eq!(
        calimero_node::usage::payer(&kept),
        format!("account:{owner}")
    );
    // Its owner deletes it, as any app.
    let _ok = node
        .sys_write(&SystemCmd::DeleteApp {
            app_id: app.clone(),
            actor: owner,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await
        .unwrap();
    let mut deleted = false;
    for _ in 0..100 {
        if node.app(&app).await.is_err() {
            deleted = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(deleted, "the app was not deleted");
    // What the old release logged without session proofs replayed as it
    // applied (the owner's account and app above), and the node on this
    // release requires the proofs from here on.
    let mut required = None;
    for _ in 0..100 {
        required = node.sys(|s| s.session_proofs_from);
        if required.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let from = required.expect("the leader requires session proofs");
    assert!(
        node.sys(|s| s.proves_sessions(from) && !s.proves_sessions(from - 1)),
        "from {from} on, not before"
    );
    // The store had no format row (format 0, every release up to 1.0.0-rc.15): opening it
    // marked it current, a row of the node's own (`merod inspect` lists it).
    assert_eq!(
        node.engine.get_node_meta::<u32>("format").unwrap(),
        Some(calimero_log::STORE_FORMAT)
    );
    r.stop().await;
}
