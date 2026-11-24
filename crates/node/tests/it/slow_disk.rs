//! The node's own store writes (blobs, bundles, segments, node rows) leave the
//! async workers.
//!
//! RocksDB writes in groups: a write that arrives while another one is in its
//! fsync joins or queues behind it, whatever group it is for. Every Raft group
//! of a node, the system group and HTTP share the async workers, so a blob put
//! left on a worker stops all of them for as long as the disk takes
//! (`calimero_log`'s `tests/slow_disk.rs` covers the log's own writes).
//!
//! `calimero_log::testing::slow_synced_writes` makes a synced write of the
//! node's own rows hold the store's write path, as a slow fsync holds
//! RocksDB's write group. The runtime has one worker, so a put that waits for
//! the disk on it stops a task ticking beside it. Told by order, not latency: while that
//! one held write still waits, the ticker must keep ticking.

// The test holds the disk with a write of the node's own rows, made directly.
#![allow(clippy::disallowed_methods)]

use crate::common;

use std::future::Future;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_log::testing;
use calimero_node::node::Node;
use common::Cluster;

/// The group id of the node's own rows in `calimero-log` (`store::NODE_ROWS`).
const NODE_ROWS: u64 = u64::MAX;
/// How long the held write holds the disk at most: far past any write's own
/// disk time, so a worker that waits for it cannot tick first.
const HOLD: Duration = Duration::from_secs(5);
/// How long the test watches the ticker while the put waits.
const WATCH: Duration = Duration::from_millis(300);
/// The fewest 10 ms ticks a free worker makes in `WATCH`.
const MIN_TICKS: u64 = 10;

/// While a synced write of the node's own rows holds its store's write path,
/// `op` (a write of the node) waits for it; this test's only worker must keep
/// running meanwhile. Answers what `op` answered once the disk was released.
async fn worker_runs_while_held<T, F, Fut>(node: &Arc<Node>, dir: &Path, op: F) -> T
where
    T: Send + 'static,
    F: FnOnce(Arc<Node>) -> Fut,
    Fut: Future<Output = T> + Send + 'static,
{
    testing::slow_synced_writes(dir, NODE_ROWS, HOLD);
    let holder = {
        let node = Arc::clone(node);
        std::thread::spawn(move || node.engine.put_node_meta("slow-disk-test", &1_u8))
    };
    let at = Instant::now();
    let held = loop {
        if let Some(n) = testing::synced_write_waiting(dir, NODE_ROWS) {
            break n;
        }
        assert!(at.elapsed() < HOLD, "the node row's write never waited");
        std::thread::sleep(Duration::from_millis(1));
    };

    let ticking = Arc::new(AtomicBool::new(true));
    let ticks = Arc::new(AtomicU64::new(0));
    let ticker = {
        let (ticking, ticks) = (Arc::clone(&ticking), Arc::clone(&ticks));
        tokio::spawn(async move {
            while ticking.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _n = ticks.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    // On the worker, as a request handler's write is (this test's body runs on
    // the thread that blocks on it, not on a worker).
    let write = tokio::spawn(op(Arc::clone(node)));
    // This thread sleeps, not the runtime: a worker that blocks would stop
    // the runtime's timers too.
    std::thread::sleep(Duration::from_millis(50));
    let before = ticks.load(Ordering::Relaxed);
    std::thread::sleep(WATCH);
    let during = ticks.load(Ordering::Relaxed) - before;
    let still_held = testing::synced_write_waiting(dir, NODE_ROWS) == Some(held);
    let waited = !write.is_finished();
    eprintln!("the worker ticked {during} times in {WATCH:?} while the write waited");

    testing::heal_synced_writes(dir, NODE_ROWS);
    holder.join().unwrap().unwrap();
    let out = write.await.unwrap();
    ticking.store(false, Ordering::Relaxed);
    ticker.await.unwrap();

    assert!(still_held, "the held write ended before the test looked");
    assert!(waited, "the write did not wait for the held disk");
    assert!(
        during >= MIN_TICKS,
        "the async worker ticked {during} times in {WATCH:?} while a write waited for the disk \
         (at least {MIN_TICKS}): the write blocked it"
    );
    out
}

/// A blob upload (`POST /apps/{app}/blobs` stores through `Node::store_blob`)
/// and a bundle (`POST /bundles`, `Node::replicate_bundle`) waiting for a slow
/// disk leave the worker to the node's other work.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_blob_put_waiting_for_the_disk_leaves_the_async_worker_free() {
    let c = Cluster::start(1).await;
    let node = Arc::clone(&c.nodes[0].as_ref().unwrap().node);
    let dir = c.configs[0].data_dir.clone();

    let id = worker_runs_while_held(&node, &dir, |node| async move {
        node.store_blob(b"bytes behind a slow disk", None).await
    })
    .await
    .unwrap();
    assert_eq!(
        node.engine.get_blob(&id).unwrap().as_deref(),
        Some(&b"bytes behind a slow disk"[..]),
        "the blob is stored once the disk answers"
    );

    let wasm = b"a bundle behind a slow disk".to_vec();
    let hash = hex::encode(calimero_identity::sha256(&wasm));
    let stored = hash.clone();
    worker_runs_while_held(&node, &dir, |node| async move {
        node.replicate_bundle(&stored, &wasm).await
    })
    .await
    .unwrap();
    assert!(node.engine.get_blob(&hash).unwrap().is_some());
}
