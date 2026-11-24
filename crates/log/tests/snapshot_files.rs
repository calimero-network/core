//! The snapshot directory holds only the files the groups serve: a receive or a
//! build a crash cut short is cleaned when the engine opens, and a build that
//! ends after its group was removed leaves nothing behind.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_log::store::Db;
use calimero_log::testing::{build_waiting, hold_build, release_build, BuildPoint};
use calimero_log::{
    BoxFuture, Config, Engine, Group, GroupId, Machine, NodeId, Proposal, RpcRequest, RpcResponse,
    StateTx, StoreWatch, Transport,
};

const KEY: [u8; 32] = [5; 32];
const GROUP: GroupId = 1;

struct Nowhere;

impl Transport for Nowhere {
    fn send(
        &self,
        _target: NodeId,
        _addr: String,
        _group: GroupId,
        _request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>> {
        Box::pin(async { Err("single node".to_owned()) })
    }
}

/// An in-memory machine (checkpointed with each snapshot) that counts entries.
#[derive(Default)]
struct Count(u64);

impl Machine for Count {
    const IN_MEMORY: bool = true;

    fn apply(&mut self, _index: u64, _command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
        self.0 += 1;
        Vec::new()
    }

    fn snapshot(&self) -> eyre::Result<Vec<u8>> {
        Ok(self.0.to_be_bytes().to_vec())
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        self.0 = data.try_into().map_or(0, u64::from_be_bytes);
        Ok(())
    }
}

fn open(dir: &Path) -> Arc<Engine> {
    let config = Config {
        heartbeat_ms: 50,
        election_min_ms: 150,
        election_max_ms: 300,
        snapshot_every: 1000,
    };
    Engine::open(1, dir, &KEY, Arc::new(Nowhere), config).unwrap()
}

/// A one-node group at `dir` that has applied a few entries.
async fn started(engine: &Engine) -> Arc<Group<Count>> {
    let group = engine.start_group(GROUP, Count::default()).await.unwrap();
    group
        .initialize(BTreeMap::from([(1, "node-1".to_owned())]))
        .await
        .unwrap();
    let mut last = 0;
    for _ in 0..5 {
        for _ in 0..100 {
            match group.propose(|_, _| Proposal::Append(b"+1".to_vec())).await {
                Ok(p) => {
                    last = p.index;
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    }
    assert!(group.wait_applied(last, Duration::from_secs(5)).await);
    group
}

fn snapshot_files(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir.join("snapshots"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

async fn closed(watch: StoreWatch) {
    while watch.is_open() {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A crash leaves a snapshot being received, a build that never recorded its
/// file, and the file of a group whose data is gone. Opened again, the engine
/// keeps only the file the group serves, and anything that is not a snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restart_cleans_the_snapshot_files_a_crash_left() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    let group = started(&engine).await;
    let applied = group.applied();
    group
        .snapshot_through(applied, Duration::from_secs(5))
        .await
        .unwrap();
    let served = engine.snapshot_file(GROUP).unwrap().unwrap().2;
    let served = served.file_name().unwrap().to_string_lossy().into_owned();
    let watch = engine.store_watch();
    engine.shutdown().await;
    drop((group, engine));
    closed(watch).await;

    let snapshots = dir.path().join("snapshots");
    let leftovers = [
        format!("g{GROUP}-recv-0a1b2c3d4e5f.snap"),
        format!("g{GROUP}-{applied}-7-0a1b2c3d4e5f.snap"),
        "g9-3-1-0a1b2c3d4e5f.snap".to_owned(),
    ];
    for name in &leftovers {
        std::fs::write(snapshots.join(name), b"cut short").unwrap();
    }
    std::fs::write(snapshots.join("README"), b"not a snapshot").unwrap();

    let engine = open(dir.path());
    assert_eq!(
        snapshot_files(dir.path()),
        vec!["README".to_owned(), served.clone()],
        "only the served snapshot (and what is not a snapshot) is left"
    );
    let group = started(&engine).await;
    assert_eq!(
        engine
            .snapshot_file(GROUP)
            .unwrap()
            .unwrap()
            .2
            .file_name()
            .unwrap()
            .to_string_lossy(),
        served,
        "the group still serves its snapshot"
    );
    engine.shutdown().await;
    drop(group);
}

/// A build held before it reads the state while its group is removed: once it
/// runs on, it records no snapshot, writes no checkpoint and leaves no file of
/// the removed group.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_build_that_ends_after_its_group_was_removed_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let engine = open(dir.path());
    let group = started(&engine).await;
    hold_build(dir.path(), GROUP, BuildPoint::Read);
    group.snapshot_now().await.unwrap();
    let held_since = Instant::now();
    while !build_waiting(dir.path(), GROUP) {
        assert!(
            held_since.elapsed() < Duration::from_secs(5),
            "no build started"
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    engine.remove_group(GROUP).await.unwrap();
    release_build(dir.path(), GROUP);
    // The build runs on its own task; give it time to write what it would.
    tokio::time::sleep(Duration::from_millis(500)).await;

    assert_eq!(
        snapshot_files(dir.path()),
        Vec::<String>::new(),
        "no snapshot file of the removed group"
    );
    assert!(engine.snapshot_file(GROUP).unwrap().is_none());
    let watch = engine.store_watch();
    drop((group, engine));
    closed(watch).await;
    let summary = Db::open(dir.path(), &KEY).unwrap().summary().unwrap();
    assert!(
        !summary.groups.contains_key(&GROUP),
        "rows of the removed group: {:?}",
        summary.groups.get(&GROUP)
    );
}
