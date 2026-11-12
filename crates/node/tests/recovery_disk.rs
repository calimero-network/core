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
    clippy::result_large_err,
    reason = "the helpers pass on calimero_client::Error as the client returns it"
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use calimero_client::types::ViewsRequestConsistency;
use calimero_client::{Client, Policy};
use calimero_node::recovery_disk::{self as disk, Kind};
use calimero_node::system::{Authority, BackupHolder};
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{kv_wasm, operator_sign, signed_up, typed, wire, Cluster};

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

/// Deploy the kv fixture as `cl`; answers the app's id.
async fn deploy(cl: &Client) -> String {
    let bundle = cl.packages().bundle_put(kv_wasm()).await.unwrap().hash;
    let created = cl
        .apps()
        .create(typed(json!({ "name": "kv", "bundle": bundle })))
        .await
        .map(wire)
        .unwrap();
    created["appId"].as_str().unwrap().to_owned()
}

async fn put(cl: &Client, app: &str, key: &str, value: &str) {
    let _i = cl
        .apps()
        .call(
            app,
            "set",
            Some(json!({ "key": key, "value": value })),
            None,
            None,
        )
        .await
        .unwrap();
}

async fn get(cl: &Client, app: &str, key: &str) -> Result<Value, calimero_client::Error> {
    cl.apps()
        .view(
            app,
            "get",
            Some(json!({ "key": key })),
            Some(ViewsRequestConsistency::Strong),
            None,
        )
        .await
        .map(|v| v.result)
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
    let kv = deploy(&alice).await;
    for (k, v) in [("a", "1"), ("b", "2")] {
        put(&alice, &kv, k, v).await;
    }

    // Without a key nothing is written.
    let survivor = old.nodes[1].as_ref().unwrap().node.clone();
    let pass = survivor.write_recovery().await.unwrap();
    assert_eq!(pass.written, 0);
    assert!(pass.without_key >= 1, "{pass:?}");

    let (secret, public) = disk::new_keypair();
    let set = ops(&old.urls[0])
        .await
        .ops()
        .set_backup_key(&hex::encode(public), Some(&operator_sign))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(set["holder"], json!({ "kind": "operator" }));
    assert_eq!(set["apps"], 1);
    // An owner cannot set a key where the operator upgrades apps.
    let e = alice
        .auth()
        .account_set_backup_key(&hex::encode(public), None)
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
        .ops()
        .recover(typed(wrong), Some(typed(json!(code))))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_RECOVERY_FILE"), "{e}");
    let out = op
        .ops()
        .recover(
            typed(opened(&disk_at, &system, &secret)),
            Some(typed(json!(code))),
        )
        .await
        .map(wire)
        .unwrap();
    assert_eq!(out["apps"], 1, "{out}");
    // Once is all: the state is no longer empty.
    let e = op
        .ops()
        .recover(
            typed(opened(&disk_at, &system, &secret)),
            Some(typed(json!(code))),
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
    let e = get(&cl, &kv, "a").await.unwrap_err();
    assert_eq!(e.code(), Some("APP_RESTORING"), "{e}");

    let restored = op
        .ops()
        .restore(
            &kv,
            None,
            Some(typed(opened(&disk_at, &app_file, &secret))),
            Some(&operator_sign),
        )
        .await
        .map(wire)
        .unwrap();
    assert_eq!(
        restored["status"]["voters"].as_array().unwrap().len(),
        3,
        "{restored}"
    );
    for (k, v) in [("a", "1"), ("b", "2")] {
        let got = get(&cl, &kv, k).await.unwrap();
        assert_eq!(got, json!(v), "{k}");
    }
    put(&cl, &kv, "after", "1").await;

    founder.stop().await;
    for r in others {
        r.stop().await;
    }
}

/// `POST /recover/holder` with `body`'s files, signed by the operator when `signed`.
async fn recover_holder(
    cl: &Client,
    body: &Value,
    signed: bool,
) -> Result<calimero_client::types::RestoreRecoverHolderResponse, calimero_client::Error> {
    cl.public()
        .restore_recover_holder(
            body["holder"].as_str().unwrap(),
            typed(body["slice"].clone()),
            Some(typed(body["apps"].clone())),
            Some(typed(body["code"].clone())),
            signed.then_some(&operator_sign as &calimero_client::OperatorSigner<'_>),
        )
        .await
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

/// Where owners upgrade apps, an account's own apps (its personal
/// organization's, made at sign-up) open with that organization's key, which
/// the account sets: the operator holds none. On a new cluster, the account's
/// device opens its files and sends their keys; the operator admits the import
/// with its signature; the organization, the account, its passkey and its app
/// come back, and it signs in with the passkey it had.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_accounts_apps_come_back_with_its_own_key_on_a_new_cluster() {
    let mut old = Cluster::start(3).await;
    let (alice, mut pk) = signed_up(&old, 0, "alice").await;
    let kv = deploy(&alice).await;
    put(&alice, &kv, "a", "1").await;
    let account = alice.auth().me().await.map(wire).unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let mine = alice.personal_org().await.unwrap();
    let holder = BackupHolder::Org(mine.clone());
    // A sealed file of the app: its segments go to the disk beside the app's
    // file, and its data key comes back wrapped under the new cluster's key.
    let plain: Vec<u8> = (0..5_000_u32).map(|i| (i % 251) as u8).collect();
    let uploaded = alice
        .upload_file(&kv, std::io::Cursor::new(plain.clone()), plain.len() as u64)
        .await
        .unwrap();
    let file_id = uploaded["id"].as_str().unwrap().to_owned();

    let (secret, public) = disk::new_keypair();
    let set = alice
        .orgs()
        .set_backup_key(&mine, &hex::encode(public), Some(&mut pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(set["holder"], json!({ "kind": "org", "id": mine }));
    assert_eq!(set["apps"], 1);
    // Here the owner upgrades apps: the operator holds no key.
    let e = ops(&old.urls[0])
        .await
        .ops()
        .set_backup_key(&hex::encode(public), Some(&operator_sign))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("FORBIDDEN"), "{e}");

    let survivor = old.nodes[2].as_ref().unwrap().node.clone();
    let mut written = 0;
    for _ in 0..100 {
        written += survivor.write_recovery().await.unwrap().written;
        if written >= 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(
        written >= 4,
        "the organization's slice, its app, code and a segment: {written}"
    );
    let disk_copy = tempfile::tempdir().unwrap();
    copy_dir(
        &survivor.config.recovery_dir.clone().unwrap(),
        disk_copy.path(),
    );
    for i in 0..3 {
        old.stop(i).await;
    }

    // Another cluster, which never held the account; the disk is attached to
    // its first node.
    let copy = disk_copy.path().to_owned();
    let new = Cluster::start_with(3, move |c| {
        if c.node_id == 1 {
            c.recovery_dir = Some(copy.clone());
        }
    })
    .await;
    let at = disk::holder_dir(disk_copy.path(), &holder);
    let code: Vec<Value> = disk::files_of(&at, "code-")
        .unwrap()
        .iter()
        .map(|p| opened(&at, &p.file_name().unwrap().to_string_lossy(), &secret))
        .collect();
    let body = json!({
        "holder": holder.key(),
        "slice": opened(&at, &newest(&at, "holder."), &secret),
        "apps": [opened(&at, &newest(&at, &format!("app-{kv}.")), &secret)],
        "code": code,
    });
    let device = new.client(0);
    // The cluster has not admitted it yet.
    let e = recover_holder(&device, &body, false).await.unwrap_err();
    assert_eq!(e.code(), Some("OPERATOR_SIGNATURE_REQUIRED"), "{e}");
    let out = recover_holder(&device, &body, true)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(
        out["apps"][0]["status"]["voters"].as_array().unwrap().len(),
        3,
        "{out}"
    );
    // Once: the account is here now.
    let e = recover_holder(&device, &body, true).await.unwrap_err();
    assert!(e.to_string().contains("holds that"), "{e}");

    // The file is back, its segment handed to every replica, and its data
    // key opens under this cluster's file key.
    let founder = new.nodes[0].as_ref().unwrap().node.clone();
    let wrapped = founder
        .hosted(&kv)
        .unwrap()
        .read_local(|m, _| {
            m.state
                .component
                .blobs
                .records
                .values()
                .find_map(|r| r.file.as_ref().map(|f| f.wrapped_key.0.clone()))
        })
        .expect("the restored app holds its file");
    let file_key = calimero_node::backup::file_key(founder.cluster_key(), &kv);
    assert!(calimero_node::files::unwrap_key(&file_key, &kv, &wrapped).is_ok());

    let cl = new.client(1);
    let _session = cl.sign_in(&mut pk).await.unwrap();
    let info = cl.apps().file_info(&kv, &file_id).await.map(wire).unwrap();
    assert_eq!(info["size"], json!(5_000));
    assert_eq!(
        cl.auth().me().await.map(wire).unwrap()["accountId"],
        account
    );
    let got = get(&cl, &kv, "a").await.unwrap();
    assert_eq!(got, json!("1"));
    put(&cl, &kv, "after", "1").await;
    // The organization's files keep going to the key they were sealed to.
    let key = cl.orgs().backup_key(&mine).await.map(wire).unwrap();
    assert_eq!(key["key"]["publicKey"], hex::encode(public));
}
