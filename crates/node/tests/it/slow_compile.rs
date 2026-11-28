//! A package install compiles each component of the release (`verify_release`),
//! and cranelift holds the thread it compiles on for seconds on a large
//! (componentize-js) component. Every Raft group of a node, the system group and
//! HTTP share the async workers, so a compile left on one stops all of them.
//!
//! `calimero_node::testing::slow_compiles` makes each install compile hold its
//! thread longer. The runtime has one worker, so a compile on it stops a task
//! ticking beside it. Told by order, not latency: while the install still
//! compiles, the ticker must keep ticking. The watch starts when the install is
//! spawned, so what the install does on the worker before its compile (a `.mpk`
//! verified there held it 50-75 ms idle, up to 300 ms loaded, when that ran on the worker) is
//! watched too, and the longest gap between ticks is judged: a worker the install
//! holds shows a gap as long as the hold.

use crate::common;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{mpk_with, Cluster, PUBLISHER};

/// How long each compile holds its thread: far past the watch, so a worker that
/// compiles cannot tick first.
const COMPILE: Duration = Duration::from_secs(2);
/// How long the test watches the ticker, from the install's spawn, while it compiles.
const WATCH: Duration = Duration::from_millis(300);
/// Both tickers' period.
const TICK: Duration = Duration::from_millis(10);
/// The longest the worker may go without a tick during the watch, beyond the
/// longest a thread of its own did (a runner that stalls both stalls them alike).
/// Measured on four cores, idle and beside six busy loops (20 runs): the free
/// worker's longest gap was 11-29 ms, the free thread's 10-33 ms; a `.mpk` verified
/// on the worker gave 52-61 ms idle beside a free thread's 10-12 ms,
/// and up to 324 ms loaded; a compile on it gives the whole `WATCH`.
const MAX_EXTRA_GAP: Duration = Duration::from_millis(30);
/// A free worker ticks at least this share of what a thread of its own ticks in
/// `WATCH`: a loaded runner slows both, a blocked worker alone.
const MIN_SHARE: f64 = 0.5;

/// An install whose components compile slowly leaves the one async worker to
/// the node's other work, and still installs once they compile.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn an_install_that_compiles_slowly_leaves_the_async_worker_free() {
    let wasm = common::component("fixture-kv");
    let c = Cluster::start(1).await;
    let node = Arc::clone(&c.nodes[0].as_ref().unwrap().node);
    let key = ed25519_dalek::SigningKey::from_bytes(&PUBLISHER);
    let mpk: Arc<[u8]> = mpk_with("com.test.slow-compile", "1.0.0", &wasm, None, &key).into();

    calimero_node::testing::slow_compiles(&node.config.public_url, COMPILE);
    let ticking = Arc::new(AtomicBool::new(true));
    let ticks = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let ticker = {
        let (ticking, ticks) = (Arc::clone(&ticking), Arc::clone(&ticks));
        tokio::spawn(async move {
            while ticking.load(Ordering::Relaxed) {
                tokio::time::sleep(TICK).await;
                ticks.lock().unwrap().push(Instant::now());
            }
        })
    };
    let control = Arc::new(Mutex::new(Vec::<Instant>::new()));
    let control_thread = {
        let (ticking, control) = (Arc::clone(&ticking), Arc::clone(&control));
        std::thread::spawn(move || {
            while ticking.load(Ordering::Relaxed) {
                std::thread::sleep(TICK);
                control.lock().unwrap().push(Instant::now());
            }
        })
    };
    // Both tick before the install starts.
    while ticks.lock().unwrap().is_empty() || control.lock().unwrap().is_empty() {
        std::thread::sleep(TICK);
    }
    // On the worker, as a request handler's install is (this test's body runs
    // on the thread that blocks on it, not on a worker).
    let install = {
        let node = Arc::clone(&node);
        tokio::spawn(async move {
            node.install_upload(mpk, calimero_node::system::OPERATOR, None)
                .await
        })
    };
    // This thread sleeps, not the runtime: a worker that blocks would stop the
    // runtime's timers too. The watch starts with the install.
    let start = Instant::now();
    std::thread::sleep(WATCH);
    let end = Instant::now();
    let compiling = !install.is_finished();
    let (during, worker_gap) = watched(&ticks, start, end);
    let (free, free_gap) = watched(&control, start, end);
    eprintln!(
        "the worker ticked {during} times (longest gap {worker_gap:?}), a free thread {free} \
         (longest gap {free_gap:?}), in {WATCH:?} while the install compiled"
    );

    let installed = install.await.unwrap();
    ticking.store(false, Ordering::Relaxed);
    ticker.await.unwrap();
    control_thread.join().unwrap();

    assert!(compiling, "the install did not wait for its compile");
    assert!(
        worker_gap < free_gap + MAX_EXTRA_GAP,
        "the async worker went {worker_gap:?} without a tick while an install compiled, \
         a free thread {free_gap:?} (at most {MAX_EXTRA_GAP:?} more): the install held it"
    );
    assert!(
        free > 0 && during as f64 >= free as f64 * MIN_SHARE,
        "the async worker ticked {during} times in {WATCH:?} while an install compiled, \
         a free thread {free}: the install blocked it"
    );
    let installed = serde_json::to_value(installed.expect("the release installs once it compiles"))
        .expect("serializes");
    assert_eq!(installed["package"], "com.test.slow-compile");
}

/// How many of `ticks` fell in `start..=end`, and the longest gap between them,
/// counting from `start` to the first and from the last to `end`.
fn watched(ticks: &Mutex<Vec<Instant>>, start: Instant, end: Instant) -> (usize, Duration) {
    let seen: Vec<Instant> = ticks
        .lock()
        .unwrap()
        .iter()
        .copied()
        .filter(|t| *t > start && *t <= end)
        .collect();
    let mut edges = vec![start];
    edges.extend(&seen);
    edges.push(end);
    let longest = edges
        .windows(2)
        .map(|w| w[1] - w[0])
        .max()
        .unwrap_or(end - start);
    (seen.len(), longest)
}

/// How long each compile holds its thread in
/// [`one_account_filling_the_compile_queue_leaves_room_for_another`].
const FAIR_COMPILE: Duration = Duration::from_millis(500);

/// One account that sends more compiles at once than the queue holds takes
/// one place in it, not all of them: another account's compile still waits its
/// turn and compiles, where before it was refused `503 BUSY`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_account_filling_the_compile_queue_leaves_room_for_another() {
    use calimero_node::compile::{Engine, MAX_CONCURRENT_COMPILES, MAX_QUEUED_COMPILES};

    let wasm = common::component("fixture-kv");
    let c = Cluster::start(1).await;
    let node = Arc::clone(&c.nodes[0].as_ref().unwrap().node);
    calimero_node::testing::slow_compiles(&node.config.public_url, FAIR_COMPILE);

    let flood: Vec<_> = (0..=MAX_CONCURRENT_COMPILES + MAX_QUEUED_COMPILES)
        .map(|i| {
            let (node, wasm) = (Arc::clone(&node), wasm.clone());
            tokio::spawn(async move {
                node.compile(Engine::App, "busy-account", &format!("busy-{i}"), wasm)
                    .await
                    .map(|_| ())
            })
        })
        .collect();
    // Every one of the flood has asked before the other account does.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let other = node
        .compile(Engine::App, "other-account", "other", wasm.clone())
        .await
        .map(|_| ());
    let mut refused = Vec::new();
    for f in flood {
        if let Err(e) = f.await.unwrap() {
            refused.push((e.status, e.code));
        }
    }
    eprintln!("the busy account's refusals: {refused:?}");
    assert!(
        other.is_ok(),
        "another account's compile was refused while one account filled the queue: {:?}",
        other.err().map(|e| (e.status, e.code))
    );
    assert!(
        !refused.is_empty(),
        "every compile of the account that filled the queue was taken"
    );
}
