//! A store its watch reports closed has released its lock.

use std::sync::Arc;
use std::time::Duration;

use calimero_log::{
    testing, BoxFuture, Config, Engine, GroupId, NodeId, RpcRequest, RpcResponse, Transport,
};

const KEY: [u8; 32] = [9; 32];
const CLOSE_DELAY: Duration = Duration::from_millis(300); // a close that has not finished when the watcher looks

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

fn open(dir: &std::path::Path) -> eyre::Result<Arc<Engine>> {
    let config = Config {
        heartbeat_ms: 50,
        election_min_ms: 150,
        election_max_ms: 300,
        snapshot_every: 1000,
    };
    Engine::open(1, dir, &KEY, Arc::new(Nowhere), config)
}

/// The last holder of the store is a group's task on another thread, so the
/// watcher can look while that thread is still closing the store.
#[test]
fn a_store_reported_closed_can_be_reopened_at_once() {
    let dir = tempfile::tempdir().unwrap();
    testing::slow_close(dir.path(), CLOSE_DELAY);
    let engine = open(dir.path()).unwrap();
    let watch = engine.store_watch();
    let last_holder = std::thread::spawn(move || drop(engine));
    while watch.is_open() {
        std::thread::sleep(Duration::from_millis(1));
    }
    let reopened = open(dir.path());
    last_holder.join().unwrap();
    if let Err(e) = reopened {
        panic!("the store was reported closed but is still locked: {e}");
    }
}
