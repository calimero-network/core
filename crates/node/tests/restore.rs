//! Restoring an app from its sealed backup (`POST /ops/apps/{app}/restore`,
//! `meroctl ops restore`): the operator's signed decision, run on the node
//! that holds the backup. The app moves to a new group seeded from the
//! backup, every replica holds the restored state, and writes go on; what the
//! app wrote after the backup is gone. A backup that another cluster wrote,
//! that belongs to another app, that a newer release wrote or that is in a
//! format this release does not read is refused before anything changes.
//!
//! Builds the component kv fixture `apps/v2-fixtures/kv` from this checkout
//! (see `harness/cluster.rs`).

#![allow(
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]

use std::path::PathBuf;
use std::time::Duration;

use calimero_client::{Client, Consistency};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, Cluster};

async fn deploy(cl: &Client, name: &str) -> String {
    let bundle = cl.upload_bundle(kv_wasm()).await.unwrap();
    let created = cl
        .post("apps", json!({ "name": name, "bundle": bundle }))
        .await
        .unwrap();
    created["appId"].as_str().unwrap().to_owned()
}

async fn set(cl: &Client, app: &str, key: &str, value: &str) -> u64 {
    cl.call(app, "set", json!({ "key": key, "value": value }), None)
        .await
        .unwrap()
        .index
}

/// `key` in `app` as node `i`'s own replica holds it, once it applied `min_index`.
async fn on_replica(
    c: &Cluster,
    i: usize,
    session: &Client,
    app: &str,
    key: &str,
    min_index: u64,
) -> Value {
    let cl = c.client(i);
    cl.use_session(session.session().await.unwrap()).await;
    cl.view(
        app,
        "get",
        json!({ "key": key }),
        Consistency::Stale { min_index },
    )
    .await
    .unwrap()
    .result
}

/// A client of node `i` holding the operator's token.
async fn ops_at(c: &Cluster, i: usize) -> Client {
    let cl = c.client(i);
    cl.use_api_key("ops-secret").await;
    cl
}

/// The node of `c` leading `app`, once one does.
async fn leader_of(c: &Cluster, app: &str) -> usize {
    for _ in 0..200 {
        if let Some(i) = c.app_leader(app) {
            return i;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{app} has no leader");
}

/// Back `app` up on its leader; answers that node and the backup's file name.
/// The leader backs up every app it leads, so a backup of `app` may be there
/// already from backing up another app it leads, and this call writes none.
async fn back_up(c: &Cluster, app: &str) -> (usize, String) {
    let at = leader_of(c, app).await;
    let node = &c.nodes[at].as_ref().unwrap().node;
    let _written = node.backup().await.unwrap();
    let mut names: Vec<String> = std::fs::read_dir(backup_dir(c, at).join(app))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().into_string().unwrap())
        .filter(|n| n.ends_with(".snap"))
        .collect();
    names.sort();
    (at, names.pop().expect("a backup of the app on its leader"))
}

fn backup_dir(c: &Cluster, i: usize) -> PathBuf {
    c.nodes[i]
        .as_ref()
        .unwrap()
        .node
        .config
        .backup_dir
        .clone()
        .unwrap()
}

async fn restore(
    c: &Cluster,
    at: usize,
    app: &str,
    body: Value,
) -> Result<Value, calimero_client::Error> {
    ops_at(c, at)
        .await
        .post_operator_signed(
            &format!("ops/apps/{app}/restore"),
            body,
            &harness::operator_sign,
        )
        .await
}

/// Back up, keep writing, restore: every replica holds the backup's state,
/// not what came after it, in a new group, and the app takes writes again.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_app_is_restored_from_its_backup_on_every_replica_and_takes_writes() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let kv = deploy(&alice, "kv").await;
    let _i = set(&alice, &kv, "a", "1").await;
    let _i = set(&alice, &kv, "b", "2").await;
    let (at, name) = back_up(&c, &kv).await;
    let before = c.nodes[at].as_ref().unwrap().node.hosted(&kv).unwrap().id();
    // After the backup: lost by the restore.
    let _i = set(&alice, &kv, "a", "changed").await;
    let _i = set(&alice, &kv, "c", "3").await;

    // An operator decision: the ops token alone is not enough.
    let e = ops_at(&c, at)
        .await
        .post(&format!("ops/apps/{kv}/restore"), json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let details = e.details().unwrap();
    assert_eq!(details["method"], "__restore_app");
    // `args` is the exact JSON text the operator signs.
    let args: Value = serde_json::from_str(details["args"].as_str().unwrap()).unwrap();
    assert_eq!(args["backup"], json!(name));

    let out = restore(&c, at, &kv, json!({ "backup": name }))
        .await
        .unwrap();
    assert_eq!(out["backup"]["name"], json!(name));
    assert_eq!(out["backup"]["format"], 1);
    assert_eq!(out["backup"]["merod"], env!("CARGO_PKG_VERSION"));
    let group = out["group"].as_u64().unwrap();
    assert!(group > before, "a new group: {out}");
    assert_eq!(
        out["status"]["voters"].as_array().unwrap().len(),
        3,
        "{out}"
    );

    // The app takes writes again, and every replica holds the restored state.
    let index = set(&alice, &kv, "d", "4").await;
    for i in 0..3 {
        let node = &c.nodes[i].as_ref().unwrap().node;
        assert!(node.sys(|s| s.apps[&kv].restore.is_none()));
        assert_eq!(node.sys(|s| s.apps[&kv].group), group);
        assert_eq!(
            on_replica(&c, i, &alice, &kv, "a", index).await,
            json!("1"),
            "node {i}"
        );
        assert_eq!(
            on_replica(&c, i, &alice, &kv, "b", index).await,
            json!("2"),
            "node {i}"
        );
        assert_eq!(
            on_replica(&c, i, &alice, &kv, "c", index).await,
            Value::Null,
            "node {i}"
        );
        assert_eq!(
            on_replica(&c, i, &alice, &kv, "d", index).await,
            json!("4"),
            "node {i}"
        );
    }
    // The old group is gone from every node.
    for _ in 0..100 {
        let gone = c
            .nodes
            .iter()
            .flatten()
            .all(|r| r.node.hosted(&kv).is_some_and(|g| g.id() == group));
        if gone {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for r in c.nodes.iter().flatten() {
        assert_eq!(r.node.hosted(&kv).unwrap().id(), group);
    }
    // Each restore is a decision of its own, signed anew: the same backup
    // restores again, and drops what came after it again.
    let _i = set(&alice, &kv, "a", "again").await;
    let again = restore(&c, at, &kv, json!({ "backup": name }))
        .await
        .unwrap();
    assert!(again["group"].as_u64().unwrap() > group, "{again}");
    let v = alice
        .view(&kv, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap()
        .result;
    assert_eq!(v, json!("1"));
}

/// What is refused before anything changes: a backup sealed for another
/// app, one a newer release wrote, one in an unknown format, one the node
/// does not hold. A backup from before manifests (format 0) is still read,
/// for the group it was taken from.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_backup_is_restored_only_where_it_belongs() {
    let c = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&c, 0, "alice").await;
    let kv = deploy(&alice, "kv").await;
    let other = deploy(&alice, "other").await;
    let _i = set(&alice, &kv, "a", "1").await;
    let _i = set(&alice, &other, "a", "other").await;
    let (at, name) = back_up(&c, &kv).await;
    let dir = backup_dir(&c, at);
    let node = &c.nodes[at].as_ref().unwrap().node;
    let refused = |e: calimero_client::Error, code: &str| {
        assert_eq!(e.code(), Some(code), "{e}");
    };

    // Not here.
    let e = restore(
        &c,
        at,
        &kv,
        json!({ "backup": "00000000000000000001.2.x.snap" }),
    )
    .await
    .unwrap_err();
    refused(e, "BACKUP_NOT_FOUND");
    let e = restore(&c, at, &kv, json!({ "backup": "../other/x.snap" }))
        .await
        .unwrap_err();
    refused(e, "BAD_BACKUP");

    // Another app's backup put under this app's name: its manifest is sealed
    // for the other app.
    let (other_at, other_name) = back_up(&c, &other).await;
    let other_dir = backup_dir(&c, other_at).join(&other);
    for suffix in ["", ".manifest"] {
        let _bytes = std::fs::copy(
            other_dir.join(format!("{other_name}{suffix}")),
            dir.join(&kv).join(format!("{other_name}{suffix}")),
        )
        .unwrap();
    }
    let e = restore(&c, at, &kv, json!({ "backup": other_name }))
        .await
        .unwrap_err();
    refused(e, "BAD_BACKUP");

    // Written by a newer merod, or in a format this one does not read.
    let manifest_path = dir.join(&kv).join(format!("{name}.manifest"));
    let original = std::fs::read(&manifest_path).unwrap();
    let manifest =
        calimero_node::backup::open_manifest(node.cluster_key(), &kv, &name, &original).unwrap();
    let newer = calimero_node::backup::Manifest {
        merod: "99.0.0".into(),
        ..manifest.clone()
    };
    std::fs::write(
        &manifest_path,
        calimero_node::backup::seal_manifest(node.cluster_key(), &name, &newer).unwrap(),
    )
    .unwrap();
    let e = restore(&c, at, &kv, json!({ "backup": name }))
        .await
        .unwrap_err();
    refused(e, "BACKUP_FROM_NEWER_RELEASE");
    let unknown = calimero_node::backup::Manifest {
        format: calimero_node::backup::FORMAT + 1,
        ..manifest.clone()
    };
    std::fs::write(
        &manifest_path,
        calimero_node::backup::seal_manifest(node.cluster_key(), &name, &unknown).unwrap(),
    )
    .unwrap();
    let e = restore(&c, at, &kv, json!({ "backup": name }))
        .await
        .unwrap_err();
    refused(e, "UNKNOWN_BACKUP_FORMAT");
    // Another cluster's key sealed it.
    std::fs::write(
        &manifest_path,
        calimero_node::backup::seal_manifest(&[1; 32], &name, &manifest).unwrap(),
    )
    .unwrap();
    let e = restore(&c, at, &kv, json!({ "backup": name }))
        .await
        .unwrap_err();
    refused(e, "BAD_BACKUP");
    // Nothing changed meanwhile.
    assert!(node.sys(|s| s.apps[&kv].restore.is_none()));
    assert_eq!(
        alice
            .view(&kv, "get", json!({ "key": "a" }), Consistency::Strong)
            .await
            .unwrap()
            .result,
        json!("1")
    );

    // A backup from before manifests restores into the group it was taken from.
    std::fs::remove_file(&manifest_path).unwrap();
    let _i = set(&alice, &kv, "a", "later").await;
    let out = restore(&c, at, &kv, json!({ "backup": name }))
        .await
        .unwrap();
    assert_eq!(out["backup"]["format"], 0, "{out}");
    let v = alice
        .view(&kv, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap()
        .result;
    assert_eq!(v, json!("1"));
    // ...and not into another one: the app has moved on to a new group.
    let e = restore(&c, at, &kv, json!({ "backup": name }))
        .await
        .unwrap_err();
    refused(e, "BAD_BACKUP");
    // The other app is untouched.
    let v = alice
        .view(&other, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap()
        .result;
    assert_eq!(v, json!("other"));
}

/// Copy the backup directory `from` into `to`, as a sync to object storage does.
fn collect(from: &std::path::Path, to: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(from) else {
        return;
    };
    std::fs::create_dir_all(to).unwrap();
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            collect(&path, &target);
        } else if !target.exists() {
            let _bytes = std::fs::copy(&path, &target).unwrap();
        }
    }
}

/// A new cluster under `name`, its founder founding it again from `store`.
async fn found_again(name: &str, store: &std::path::Path) -> Cluster {
    Cluster::start_with(3, |config| {
        config.cluster = name.to_owned();
        if config.node_id == 1 {
            config.restore_from = Some(store.to_owned());
            config.backup_dir = Some(store.to_owned());
        }
    })
    .await
}

/// After founding again: alice is back through her refresh token, the app
/// waits for its backup, and once restored holds its data and takes writes.
async fn alice_and_her_app_come_back(
    new: &Cluster,
    alice: &Client,
    kv: &str,
    expect: &[(&str, &str)],
) {
    let cl = new.client(0);
    cl.use_session(alice.session().await.unwrap()).await;
    let _fresh = cl.refresh().await.unwrap();
    assert_eq!(cl.get("auth/me").await.unwrap()["name"], "alice");
    let e = cl
        .view(kv, "get", json!({ "key": "a" }), Consistency::Strong)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("APP_RESTORING"), "{e}");
    for r in new.nodes.iter().flatten() {
        assert!(
            r.node.hosted(kv).is_none(),
            "an app waiting for its backup is hosted nowhere"
        );
    }

    let out = restore(new, 0, kv, json!({})).await.unwrap();
    assert_eq!(
        out["status"]["voters"].as_array().unwrap().len(),
        3,
        "{out}"
    );
    for (key, value) in expect {
        let v = cl
            .view(kv, "get", json!({ "key": key }), Consistency::Strong)
            .await
            .unwrap()
            .result;
        assert_eq!(v, json!(value), "{key}");
    }
    let index = set(&cl, kv, "after", "1").await;
    for i in 0..3 {
        assert_eq!(
            on_replica(new, i, &cl, kv, "after", index).await,
            json!("1"),
            "node {i}"
        );
    }
}

/// A cluster that must be recreated (a one-way format change, a lost
/// region): its nodes' backups found it again on new nodes under its own
/// cluster key. Accounts, sessions' refresh tokens and the directory come
/// back from the system backup; each app comes back from its own backup. A
/// node whose storage key opens no kept copy of the cluster key (another
/// release, another cluster name) cannot found it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cluster_is_founded_again_from_its_backups() {
    let mut old = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&old, 0, "alice").await;
    let kv = deploy(&alice, "kv").await;
    let _i = set(&alice, &kv, "a", "1").await;
    let _i = set(&alice, &kv, "b", "2").await;
    // Every node backs up what it leads; together, the system group and every app.
    let store = tempfile::tempdir().unwrap();
    for i in 0..3 {
        let _n = old.nodes[i].as_ref().unwrap().node.backup().await.unwrap();
        collect(&backup_dir(&old, i), store.path());
    }
    for sub in ["system", "cluster-key", "code", kv.as_str()] {
        assert!(store.path().join(sub).is_dir(), "{sub} is backed up");
    }
    let name = old.name.clone();
    for i in 0..3 {
        old.stop(i).await;
    }

    // Another release's node: its K_r opens none of the kept copies.
    let elsewhere = tempfile::tempdir().unwrap();
    let url = format!("http://127.0.0.1:{}/", harness::free_port());
    let peers: std::collections::BTreeMap<u64, String> = [(1, url)].into_iter().collect();
    let mut config = harness::node_config(1, &peers, elsewhere.path());
    config.cluster.clone_from(&name);
    config.storage_key = [43; 32];
    config.restore_from = Some(store.path().to_owned());
    let Err(e) = calimero_node::run(config).await else {
        panic!("a node of another release founded the cluster again");
    };
    assert!(format!("{e:#}").contains("no cluster key"), "{e:#}");

    let new = found_again(&name, store.path()).await;
    let founder = &new.nodes[0].as_ref().unwrap().node;
    assert_eq!(founder.cluster_key(), &old_key(store.path()));
    alice_and_her_app_come_back(&new, &alice, &kv, &[("a", "1"), ("b", "2")]).await;
}

/// The cluster key the store keeps for the test release, as its nodes wrapped it.
fn old_key(store: &std::path::Path) -> [u8; 32] {
    calimero_node::backup::cluster_key_from(store, &[42; 32]).unwrap()
}

/// Disaster recovery: two of three nodes are lost, so the system group has no
/// majority and nothing can change. The survivor still holds every group it
/// applied; it backs them all up, leader or not (`ops/backup?all=true`), and
/// the cluster is founded again from that.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_cluster_that_lost_its_system_majority_is_founded_again_from_a_survivor() {
    let mut old = Cluster::start(3).await;
    let (alice, _pk) = signed_up(&old, 0, "alice").await;
    let kv = deploy(&alice, "kv").await;
    let _i = set(&alice, &kv, "a", "1").await;
    let last = set(&alice, &kv, "b", "2").await;
    let survivor = &old.nodes[0].as_ref().unwrap().node;
    assert!(
        survivor
            .hosted(&kv)
            .unwrap()
            .wait_applied(last, Duration::from_secs(10))
            .await
    );
    let name = old.name.clone();
    old.stop(1).await;
    old.stop(2).await;
    // Without a majority, the survivor leads nothing; it still backs up all it holds.
    let out = ops_at(&old, 0)
        .await
        .post("ops/backup?all=true", json!({}))
        .await
        .unwrap();
    assert!(out["written"].as_u64().unwrap() >= 2, "{out}");
    let store = tempfile::tempdir().unwrap();
    collect(&backup_dir(&old, 0), store.path());
    old.stop(0).await;

    let new = found_again(&name, store.path()).await;
    alice_and_her_app_come_back(&new, &alice, &kv, &[("a", "1"), ("b", "2")]).await;
}
