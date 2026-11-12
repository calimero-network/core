//! A node refuses a store older than its rollback counter has seen
//! (`calimero_node::rollback`, `docs/specs/rollback-protection.md`).
//!
//! The host's attack, as a host does it: stop the node, copy its data
//! directory, let it run on, stop it, put the copy back, start it again.
//! `MemoryCounter` stands in for mero-kms: it outlives the node's restarts.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use calimero_node::attest::Mode;
use calimero_node::rollback::{Counter, MemoryCounter, CHECKPOINT_EVERY};
use calimero_node::{run, Config, Running};

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn config(dir: &Path, node_id: u64, counter: Option<Arc<MemoryCounter>>) -> Config {
    let port = free_port();
    let url = format!("http://127.0.0.1:{port}/");
    Config {
        node_id,
        public_url: url.clone(),
        listen: format!("127.0.0.1:{port}").parse().unwrap(),
        data_dir: dir.to_owned(),
        peers: BTreeMap::from([(node_id, url)]),
        bootstrap: true,
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
        rollback: counter.map(|c| c as Arc<dyn Counter>),
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

/// Copy a stopped node's data directory, as a host copies a disk.
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            let _ = std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Put `copy` back in place of `dir`.
fn restore(copy: &Path, dir: &Path) {
    std::fs::remove_dir_all(dir).unwrap();
    copy_dir(copy, dir);
}

/// Run a node for a few checkpoints, then stop it.
async fn run_a_while(cfg: Config) {
    let running: Running = run(cfg).await.unwrap();
    tokio::time::sleep(CHECKPOINT_EVERY * 3).await;
    running.stop().await;
}

async fn refusal(cfg: Config) -> String {
    match run(cfg).await {
        Ok(running) => {
            running.stop().await;
            panic!("the node started");
        }
        Err(e) => format!("{e:#}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restored_older_data_dir_is_refused_and_a_normal_restart_starts() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("node");
    let old = tmp.path().join("old");
    let kms = Arc::new(MemoryCounter::default());

    run_a_while(config(&dir, 1, Some(Arc::clone(&kms)))).await;
    copy_dir(&dir, &old);
    // A normal restart: the store is where mero-kms last saw it.
    run_a_while(config(&dir, 1, Some(Arc::clone(&kms)))).await;
    let (_, seen) = kms.seen();
    assert!(seen.unwrap().mark >= 4, "{seen:?}");

    restore(&old, &dir);
    let why = refusal(config(&dir, 1, Some(Arc::clone(&kms)))).await;
    assert!(why.contains("rolled back"), "{why}");
    assert!(why.contains("--accept-rollback"), "{why}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_empty_store_or_another_nodes_store_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("node");
    let kms = Arc::new(MemoryCounter::default());
    run_a_while(config(&dir, 1, Some(Arc::clone(&kms)))).await;

    // The same node id on an empty store: it would rejoin with no memory of its votes.
    let empty = tmp.path().join("empty");
    let why = refusal(config(&empty, 1, Some(Arc::clone(&kms)))).await;
    assert!(why.contains("no checkpoint"), "{why}");

    // This store under another node id (a counter of its own, never raised).
    let why = refusal(config(&dir, 2, Some(Arc::new(MemoryCounter::default())))).await;
    assert!(why.contains("belongs to node 1"), "{why}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn accept_rollback_starts_and_raises_the_epoch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("node");
    let old = tmp.path().join("old");
    let kms = Arc::new(MemoryCounter::default());
    run_a_while(config(&dir, 1, Some(Arc::clone(&kms)))).await;
    copy_dir(&dir, &old);
    run_a_while(config(&dir, 1, Some(Arc::clone(&kms)))).await;
    restore(&old, &dir);

    let mut cfg = config(&dir, 1, Some(Arc::clone(&kms)));
    cfg.accept_rollback = true;
    let running = run(cfg).await.unwrap();
    let status = running.node.checkpoints.as_ref().unwrap().status().unwrap();
    assert_eq!((status.epoch, status.forced), (1, true), "{status:?}");
    running.stop().await;
    assert_eq!(kms.seen().0, 1, "the cluster epoch is raised");

    // From then on it starts as usual, in the new generation.
    let running = run(config(&dir, 1, Some(Arc::clone(&kms)))).await.unwrap();
    let status = running.node.checkpoints.as_ref().unwrap().status().unwrap();
    assert_eq!(
        (status.epoch, status.generation, status.forced),
        (1, 1, false)
    );
    running.stop().await;
}

/// Upgrade: a store a node wrote before rollback protection starts and gains a
/// counter; from then on an answer without counters is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_store_from_before_the_counter_starts_and_gains_one() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("node");
    run_a_while(config(&dir, 1, None)).await;

    let kms = Arc::new(MemoryCounter::default());
    run_a_while(config(&dir, 1, Some(Arc::clone(&kms)))).await;
    assert!(kms.seen().1.is_some(), "the first start wrote a counter");

    kms.forget_counters();
    let why = refusal(config(&dir, 1, Some(Arc::clone(&kms)))).await;
    assert!(why.contains("answered without one"), "{why}");
}
