//! A backup's file work leaves the async workers.
//!
//! A backup copies a group's snapshot file into `--backup-dir` and syncs it,
//! with its manifest, the kept code and the kept cluster key: on a slow or
//! contended disk that takes as long as the disk does. Every Raft group of the
//! node, the system group and HTTP share the async workers, so a backup left on
//! one stops all of them meanwhile.
//!
//! `calimero_node::testing::slow_backup_files` holds the thread that writes a
//! backup file, as a slow copy and fsync would. The runtime has one worker, so
//! a backup that writes on it stops a task ticking beside it. Told by order,
//! not latency: while that write still waits, the ticker must keep ticking.

use crate::common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_node::testing;
use common::Cluster;

/// How long a held backup write holds its thread at most: far past any file
/// write's own disk time, so a worker that waits for it cannot tick first.
const HOLD: Duration = Duration::from_secs(5);
/// How long the test watches the ticker while the backup waits.
const WATCH: Duration = Duration::from_millis(300);
/// The fewest 10 ms ticks a free worker makes in `WATCH`.
const MIN_TICKS: u64 = 10;

/// `POST /ops/backup` (`Node::backup_everything` here, every group) waiting for
/// a slow disk leaves the worker to the node's other work, and writes its
/// backup once the disk answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_backup_waiting_for_the_disk_leaves_the_async_worker_free() {
    let c = Cluster::start(1).await;
    let node = Arc::clone(&c.nodes[0].as_ref().unwrap().node);
    let dir = c.configs[0].backup_dir.clone().unwrap();

    testing::slow_backup_files(&dir, HOLD);
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
    // On the worker, as the ops route's backup is (this test's body runs on
    // the thread that blocks on it, not on a worker).
    let backup = {
        let node = Arc::clone(&node);
        tokio::spawn(async move { node.backup_everything().await })
    };
    // This thread waits, not the runtime: a worker that blocks would stop the
    // runtime's timers too.
    let at = Instant::now();
    while !testing::backup_file_waiting(&dir) {
        assert!(at.elapsed() < HOLD, "the backup never wrote a file");
        std::thread::sleep(Duration::from_millis(1));
    }
    let before = ticks.load(Ordering::Relaxed);
    std::thread::sleep(WATCH);
    let during = ticks.load(Ordering::Relaxed) - before;
    let still_held = testing::backup_file_waiting(&dir);
    eprintln!("the worker ticked {during} times in {WATCH:?} while the backup waited");

    testing::heal_backup_files(&dir);
    let written = backup.await.unwrap().unwrap();
    ticking.store(false, Ordering::Relaxed);
    ticker.await.unwrap();

    assert!(
        still_held,
        "the held backup write ended before the test looked"
    );
    assert!(
        during >= MIN_TICKS,
        "the async worker ticked {during} times in {WATCH:?} while a backup waited for the disk \
         (at least {MIN_TICKS}): the backup blocked it"
    );
    assert!(
        written >= 1,
        "the backup wrote no snapshot once the disk answered"
    );
}
