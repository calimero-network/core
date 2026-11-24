//! A package install compiles each component of the release (`verify_release`),
//! and cranelift holds the thread it compiles on for seconds on a large
//! (componentize-js) component. Every Raft group of a node, the system group and
//! HTTP share the async workers, so a compile left on one stops all of them.
//!
//! `calimero_node::testing::slow_compiles` makes each install compile hold its
//! thread longer. The runtime has one worker, so a compile on it stops a task
//! ticking beside it. Told by order, not latency: while the install still
//! compiles, the ticker must keep ticking.

use crate::common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use common::{mpk_with, Cluster, PUBLISHER};

/// How long each compile holds its thread: far past the watch, so a worker that
/// compiles cannot tick first.
const COMPILE: Duration = Duration::from_secs(2);
/// How long the test watches the ticker while the install compiles.
const WATCH: Duration = Duration::from_millis(300);
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
    let control = Arc::new(AtomicU64::new(0));
    let control_thread = {
        let (ticking, control) = (Arc::clone(&ticking), Arc::clone(&control));
        std::thread::spawn(move || {
            while ticking.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                let _n = control.fetch_add(1, Ordering::Relaxed);
            }
        })
    };
    // On the worker, as a request handler's install is (this test's body runs
    // on the thread that blocks on it, not on a worker).
    let install = {
        let node = Arc::clone(&node);
        tokio::spawn(async move {
            node.install_upload(mpk, calimero_node::system::OPERATOR)
                .await
        })
    };
    // This thread sleeps, not the runtime: a worker that blocks would stop the
    // runtime's timers too.
    std::thread::sleep(Duration::from_millis(50));
    let before = (
        ticks.load(Ordering::Relaxed),
        control.load(Ordering::Relaxed),
    );
    std::thread::sleep(WATCH);
    let during = ticks.load(Ordering::Relaxed) - before.0;
    let free = control.load(Ordering::Relaxed) - before.1;
    let compiling = !install.is_finished();
    eprintln!("the worker ticked {during} times, a free thread {free}, in {WATCH:?} while the install compiled");

    let installed = install.await.unwrap();
    ticking.store(false, Ordering::Relaxed);
    ticker.await.unwrap();
    control_thread.join().unwrap();

    assert!(compiling, "the install did not wait for its compile");
    assert!(
        free > 0 && during as f64 >= free as f64 * MIN_SHARE,
        "the async worker ticked {during} times in {WATCH:?} while an install compiled, \
         a free thread {free}: the install blocked it"
    );
    let installed = installed.expect("the release installs once it compiles");
    assert_eq!(installed["package"], "com.test.slow-compile");
}
