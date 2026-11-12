//! The recovery disk (`--recovery-dir`): backups sealed to the key of whoever
//! upgrades each app, which open with that key alone. Where the operator
//! upgrades apps, its key opens everything: after every mero-kms the cluster
//! used is gone (so no node's K_r, and no copy of K_c, is left), one node's
//! recovery disk founds the cluster again under a new K_c
//! (`--bootstrap --recover-from`, `POST /ops/recover`), and each app comes back
//! from its own file (`POST /ops/apps/{app}/restore { recovery }`).
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

#![allow(
    deprecated,
    reason = "the hand-written route methods, until these tests use the generated ones"
)]
#![allow(
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_client::{Client, Consistency, Policy};
use calimero_node::recovery_disk::{self as disk, Kind};
use calimero_node::system::{Authority, BackupHolder};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, operator_sign, signed_up, Cluster};

fn operator_upgrades(config: &mut calimero_node::Config) {
    config.governance.app_upgrades = Authority::Operator;
}

async fn ops(url: &str) -> Client {
    let cl = Client::connect(
        url,
        Policy {
            allow_mock: true,
            ..Policy::default()
        },
    )
    .unwrap();
    cl.use_api_key("ops-secret").await;
    cl
}

/// The newest file in `dir` whose name starts `prefix`.
fn newest(dir: &Path, prefix: &str) -> String {
    let mut names = disk::files_of(dir, prefix).unwrap();
    let last = names
        .pop()
        .unwrap_or_else(|| panic!("no {prefix} file in {}", dir.display()));
    last.file_name().unwrap().to_string_lossy().into_owned()
}

/// `{ file, key }` for `name` in `dir`: the data key the holder's secret opens.
fn opened(dir: &Path, name: &str, secret: &[u8; 32]) -> Value {
    let header = disk::read(&dir.join(name)).unwrap().header;
    let key = disk::open_key(&header, secret).unwrap();
    json!({ "file": name, "key": hex::encode(*key) })
}

/// Every mero-kms of a cluster is gone: no node's K_r, no copy of its K_c.
/// The operator, who upgrades its apps, holds the key one surviving recovery
/// disk opens with: the cluster is founded again from it under a new K_c,
/// people sign in again with what they had, and the app comes back with its
/// data and takes writes. Nothing on the disk opens without the key, and a
/// file of another app, or another holder's key, is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cluster_comes_back_from_its_recovery_disk_after_every_kms_is_gone() {
    let mut old = Cluster::start_with(3, operator_upgrades).await;
    let (alice, _pk) = signed_up(&old, 0, "alice").await;
    let bundle = alice.upload_bundle(kv_wasm()).await.unwrap();
    let kv = alice
        .post("apps", json!({ "name": "kv", "bundle": bundle }))
        .await
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned();
    for (k, v) in [("a", "1"), ("b", "2")] {
        let _i = alice
            .call(&kv, "set", json!({ "key": k, "value": v }), None)
            .await
            .unwrap();
    }

    // Without a key nothing is written.
    let survivor = old.nodes[1].as_ref().unwrap().node.clone();
    let pass = survivor.write_recovery().await.unwrap();
    assert_eq!(pass.written, 0);
    assert!(pass.without_key >= 1, "{pass:?}");

    let (secret, public) = disk::new_keypair();
    let set = ops(&old.urls[0])
        .await
        .post_operator_signed(
            "ops/backup-key",
            json!({ "publicKey": hex::encode(public) }),
            &operator_sign,
        )
        .await
        .unwrap();
    assert_eq!(set["holder"], json!({ "kind": "operator" }));
    assert_eq!(set["apps"], 1);
    // An owner cannot set a key where the operator upgrades apps.
    let e = alice
        .post(
            "auth/backup-key",
            json!({ "publicKey": hex::encode(public) }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    // The survivor's recovery disk, once it knows the key.
    let mut written = 0;
    for _ in 0..100 {
        written += survivor.write_recovery().await.unwrap().written;
        if written >= 3 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(written >= 3, "the system, the app and its code: {written}");
    let recovery: PathBuf = survivor.config.recovery_dir.clone().unwrap();
    let at = disk::holder_dir(&recovery, &BackupHolder::Operator);
    // A second pass with nothing new writes nothing.
    assert_eq!(survivor.write_recovery().await.unwrap().written, 0);
    // Only the holder's key opens a file.
    let app_file = newest(&at, &format!("app-{kv}."));
    let header = disk::read(&at.join(&app_file)).unwrap().header;
    assert_eq!(header.kind, Kind::App);
    let (stranger, _) = disk::new_keypair();
    assert!(disk::open_key(&header, &stranger).is_err());

    let name = old.name.clone();
    let disk_copy = tempfile::tempdir().unwrap();
    copy_dir(&recovery, disk_copy.path());
    for i in 0..3 {
        old.stop(i).await;
    }

    // A new cluster of the same name, governance and operator key, on a new
    // storage key: nothing of the old K_r or K_c is left.
    let ports: Vec<u16> = (0..3).map(|_| harness::free_port()).collect();
    let peers: BTreeMap<u64, String> = ports
        .iter()
        .enumerate()
        .map(|(i, p)| (i as u64 + 1, format!("http://127.0.0.1:{p}/")))
        .collect();
    let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let config = |id: u64| {
        let mut c = harness::node_config(id, &peers, dirs[id as usize - 1].path());
        c.cluster.clone_from(&name);
        c.storage_key = [44; 32];
        operator_upgrades(&mut c);
        c
    };
    let mut founder_config = config(1);
    founder_config.recover_from = Some(disk_copy.path().to_owned());
    let founder = calimero_node::run(founder_config).await.unwrap();
    let op = ops(&peers[&1]).await;

    let disk_at = disk::holder_dir(disk_copy.path(), &BackupHolder::Operator);
    let system = newest(&disk_at, "system.");
    let code: Vec<Value> = disk::files_of(&disk_at, "code-")
        .unwrap()
        .iter()
        .map(|p| opened(&disk_at, &p.file_name().unwrap().to_string_lossy(), &secret))
        .collect();
    // A key that does not open the file is refused, and nothing changes.
    let mut wrong = opened(&disk_at, &system, &secret);
    wrong["key"] = json!(hex::encode([7_u8; 32]));
    let e = op
        .post("ops/recover", json!({ "system": wrong, "code": code }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_RECOVERY_FILE"), "{e}");
    let out = op
        .post(
            "ops/recover",
            json!({ "system": opened(&disk_at, &system, &secret), "code": code }),
        )
        .await
        .unwrap();
    assert_eq!(out["apps"], 1, "{out}");
    // Once is all: the state is no longer empty.
    let e = op
        .post(
            "ops/recover",
            json!({ "system": opened(&disk_at, &system, &secret), "code": code }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_RECOVERING"), "{e}");

    let mut others = Vec::new();
    for id in [2, 3] {
        others.push(calimero_node::run(config(id)).await.unwrap());
    }
    for _ in 0..300 {
        if others.iter().all(|r| r.node.sys(|s| s.nodes.len() == 3)) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Alice is back with her refresh token; her app waits for its file.
    let cl = Client::connect(
        &peers[&1],
        Policy {
            allow_mock: true,
            ..Policy::default()
        },
    )
    .unwrap();
    cl.use_session(alice.session().await.unwrap()).await;
    let _fresh = cl.refresh().await.unwrap();
    let e = cl
        .view(&kv, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_RESTORING"), "{e}");

    let restored = op
        .post_operator_signed(
            &format!("ops/apps/{kv}/restore"),
            json!({ "recovery": opened(&disk_at, &app_file, &secret) }),
            &operator_sign,
        )
        .await
        .unwrap();
    assert_eq!(
        restored["status"]["voters"].as_array().unwrap().len(),
        3,
        "{restored}"
    );
    for (k, v) in [("a", "1"), ("b", "2")] {
        let got = cl
            .view(&kv, "get", json!({ "key": k }), Consistency::Strong)
            .await
            .unwrap()
            .result;
        assert_eq!(got, json!(v), "{k}");
    }
    let _i = cl
        .call(&kv, "set", json!({ "key": "after", "value": "1" }), None)
        .await
        .unwrap();

    founder.stop().await;
    for r in others {
        r.stop().await;
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().filter_map(Result::ok) {
        let target = to.join(entry.file_name());
        if entry.path().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            let _bytes = std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}
