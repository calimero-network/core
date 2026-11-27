//! A slow disk under one group does not hold up the node's other groups.
//!
//! Every group of a node shares the node's async workers. A synced write (a log
//! append, a vote) or snapshot file I/O run inline on one would hold it for as
//! long as the disk takes, and every other group's heartbeats, elections and
//! RPCs wait behind it. The runtime here has one worker, so any blocking call
//! shows as the whole of its delay.
//!
//! An unsynced write waits for the disk too: RocksDB writes in groups, and a
//! write that arrives while a synced one is in its fsync joins or queues
//! behind it. `testing::slow_synced_writes` holds the node's write path for its
//! delay as that fsync would, so a write left on a worker shows as a stall.
//! The stall is told by order rather than latency (a loaded runner's own fsync
//! or a descheduled thread can take as long): a synced write of the slow group
//! held on a node, the healthy group's commit reaching that node must leave
//! this test's only worker running while the same write still waits.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use calimero_log::{
    testing, BoxFuture, Config, Engine, Group, GroupId, Machine, NodeId, Proposal, RpcRequest,
    RpcResponse, StateTx, Transport, WriteError,
};

const KEY: [u8; 32] = [5; 32];
const SLOW: GroupId = 1;
const HEALTHY: GroupId = 2;
/// How long each synced write of the slow group takes: longer than the
/// election timeout's lower bound would allow a worker to stall.
const SYNC_DELAY: Duration = Duration::from_millis(250);
/// The most a 10 ms tick may run late while the slow group writes.
const MAX_LATE: Duration = Duration::from_millis(100);
/// The longest a held synced write of the slow group waits: far past any
/// write's own disk time or a descheduled thread, so a worker that waits for
/// it cannot run again first.
const HOLD: Duration = Duration::from_secs(5);

#[derive(Default)]
struct Kv(BTreeMap<String, String>);

impl Machine for Kv {
    const IN_MEMORY: bool = true;

    fn apply(&mut self, _index: u64, command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
        let s = String::from_utf8_lossy(command);
        let (k, v) = s.split_once('=').unwrap_or((&s, ""));
        let _prev = self.0.insert(k.to_owned(), v.to_owned());
        Vec::new()
    }

    fn snapshot(&self) -> eyre::Result<Vec<u8>> {
        Ok(serde_json::to_vec(&self.0)?)
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        self.0 = if data.is_empty() {
            BTreeMap::new()
        } else {
            serde_json::from_slice(data)?
        };
        Ok(())
    }
}

#[derive(Default)]
struct Net {
    engines: RwLock<HashMap<NodeId, Arc<Engine>>>,
    /// Requests to these (node, group) wait before they are delivered.
    paused: Mutex<HashSet<(NodeId, GroupId)>>,
    /// Replies from these (node, group) wait before they are returned.
    held_replies: Mutex<HashSet<(NodeId, GroupId)>>,
}

impl Net {
    async fn wait_while(set: &Mutex<HashSet<(NodeId, GroupId)>>, at: (NodeId, GroupId)) {
        while set.lock().unwrap().contains(&at) {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    fn set(set: &Mutex<HashSet<(NodeId, GroupId)>>, at: (NodeId, GroupId), on: bool) {
        let mut s = set.lock().unwrap();
        let _changed = if on { s.insert(at) } else { s.remove(&at) };
    }
}

struct Wire(Arc<Net>);

impl Transport for Wire {
    fn send(
        &self,
        target: NodeId,
        _addr: String,
        group: GroupId,
        request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>> {
        let net = Arc::clone(&self.0);
        Box::pin(async move {
            Net::wait_while(&net.paused, (target, group)).await;
            let engine = net.engines.read().unwrap().get(&target).cloned();
            let reply = match engine {
                Some(e) => Ok(e.handle_rpc(group, request).await),
                None => Err("no such node".into()),
            };
            Net::wait_while(&net.held_replies, (target, group)).await;
            reply
        })
    }
}

type Groups = HashMap<NodeId, Arc<Group<Kv>>>;

async fn leader(groups: &Groups) -> NodeId {
    for _ in 0..200 {
        if let Some(g) = groups.values().find(|g| g.status().is_leader) {
            return g.node_id();
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("no leader");
}

/// Write `key` through the group's leader; the index it was appended at.
async fn put(groups: &Groups, key: &str) -> u64 {
    for _ in 0..50 {
        let id = leader(groups).await;
        let cmd = format!("{key}=v").into_bytes();
        match groups[&id]
            .propose(|_m, _applied| Proposal::Append(cmd.clone()))
            .await
        {
            Ok(p) => return p.index,
            Err(WriteError::NotLeader { .. } | WriteError::Unavailable(_)) => {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            Err(e) => panic!("{e}"),
        }
    }
    panic!("write never succeeded");
}

fn terms(groups: &Groups) -> Vec<u64> {
    let mut t: Vec<u64> = groups.values().map(|g| g.status().term).collect();
    t.sort_unstable();
    t
}

/// One worker thread, as a node under load has none to spare: before synced
/// writes left the workers, every append of the slow group stopped the ticker
/// (and every other group) for the whole fsync. Then the commit index, an
/// unsynced write, lands on a node while a synced write of the slow group holds
/// its write path: saved on the worker, it stopped the runtime until that
/// write went on.
#[tokio::test(flavor = "current_thread")]
async fn a_slow_disk_under_one_group_does_not_stall_the_others() {
    let net = Arc::new(Net::default());
    let config = Config {
        heartbeat_ms: 50,
        election_min_ms: 300,
        election_max_ms: 600,
        snapshot_every: 1000,
        snapshot_scale_bytes: None,
    };
    let mut dirs = Vec::new();
    let mut slow = Groups::new();
    let mut healthy = Groups::new();
    for id in 1..=3 {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(
            id,
            dir.path(),
            &KEY,
            Arc::new(Wire(Arc::clone(&net))),
            config.clone(),
        )
        .unwrap();
        let _s = slow.insert(id, engine.start_group(SLOW, Kv::default()).await.unwrap());
        let _h = healthy.insert(
            id,
            engine.start_group(HEALTHY, Kv::default()).await.unwrap(),
        );
        let _prev = net.engines.write().unwrap().insert(id, engine);
        dirs.push(dir);
    }
    let voters: BTreeMap<NodeId, String> = (1..=3).map(|i| (i, format!("node-{i}"))).collect();
    slow[&1].initialize(voters.clone()).await.unwrap();
    healthy[&1].initialize(voters).await.unwrap();
    let _index = put(&slow, "warm").await;
    let _index = put(&healthy, "warm").await;
    // Let every follower learn the healthy group's leader before its terms are taken.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let healthy_terms = terms(&healthy);

    for dir in &dirs {
        testing::slow_synced_writes(dir.path(), SLOW, SYNC_DELAY);
    }
    let ticking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ticker = {
        let ticking = Arc::clone(&ticking);
        tokio::spawn(async move {
            let mut late = Duration::ZERO;
            while ticking.load(std::sync::atomic::Ordering::Relaxed) {
                let at = Instant::now();
                tokio::time::sleep(Duration::from_millis(10)).await;
                late = late.max(at.elapsed().saturating_sub(Duration::from_millis(10)));
            }
            late
        })
    };
    let started = Instant::now();
    let ((), ()) = tokio::join!(
        async {
            for i in 0..4 {
                let _index = put(&slow, &format!("slow{i}")).await;
            }
        },
        async {
            while started.elapsed() < SYNC_DELAY * 6 {
                let _index = put(&healthy, "beside").await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    );
    ticking.store(false, std::sync::atomic::Ordering::Relaxed);
    let late = ticker.await.unwrap();
    for dir in &dirs {
        testing::heal_synced_writes(dir.path(), SLOW);
    }
    assert!(
        late < MAX_LATE,
        "a worker stalled {late:?} on the slow group's disk (at most {MAX_LATE:?})"
    );

    // A follower of the healthy group appends an entry the leader does not
    // hear of (its replies held, the other follower cut off), so the entry
    // commits only later. A synced write of the slow group then holds that
    // follower's write path, and the commit reaches it: openraft saves the
    // commit index from the group's core, on the worker. The held write must
    // still be the one waiting when this test runs again.
    let lead = leader(&healthy).await;
    let mut followers = (1..=3).filter(|id| *id != lead);
    let (x, y) = (followers.next().unwrap(), followers.next().unwrap());
    let dir = dirs[usize::try_from(x).unwrap() - 1].path();
    Net::set(&net.paused, (y, HEALTHY), true);
    Net::set(&net.held_replies, (x, HEALTHY), true);
    let appended = healthy[&x].status().last_log.unwrap_or(0);
    let write = {
        let healthy = healthy.clone();
        tokio::spawn(async move { put(&healthy, "committed-later").await })
    };
    let at = Instant::now();
    while healthy[&x].status().last_log.unwrap_or(0) <= appended {
        assert!(
            at.elapsed() < HOLD,
            "node {x} never appended the healthy write"
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    testing::slow_synced_writes(dir, SLOW, HOLD);
    let held_put = {
        let slow = slow.clone();
        tokio::spawn(async move { put(&slow, "held").await })
    };
    let held = {
        let at = Instant::now();
        loop {
            if let Some(n) = testing::synced_write_waiting(dir, SLOW) {
                break n;
            }
            assert!(
                at.elapsed() < HOLD,
                "no synced write of the slow group was seen waiting on node {x}"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    };
    Net::set(&net.held_replies, (x, HEALTHY), false);
    let index = write.await.unwrap();
    // The leader tells node `x` the commit with its next heartbeat or two.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        testing::synced_write_waiting(dir, SLOW),
        Some(held),
        "this test's only worker ran again only after node {x}'s held synced write \
         went on: the healthy group's commit index ({index}) was saved on it"
    );
    testing::heal_synced_writes(dir, SLOW);
    Net::set(&net.paused, (y, HEALTHY), false);
    let _index = held_put.await.unwrap();
    for g in healthy.values() {
        assert!(
            g.wait_applied(index, HOLD).await,
            "node {} never applied the healthy write",
            g.node_id()
        );
    }

    assert_eq!(
        terms(&healthy),
        healthy_terms,
        "the healthy group elected a new leader while the other's disk was slow or held"
    );
}

/// A machine that keeps its state on the disk: every applied batch commits a
/// write (the applied index at least).
#[derive(Default)]
struct OnDisk(u64);

impl Machine for OnDisk {
    fn apply(&mut self, _index: u64, _command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
        self.0 += 1;
        Vec::new()
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        Ok(())
    }
}

/// The group id of a node's own rows (`store::NODE_ROWS`), whose synced write
/// `Engine::put_node_meta` makes.
const NODE_ROWS: GroupId = u64::MAX;

/// An apply holds the group's applied state locked while its batch commits,
/// so no reader sees a state the disk does not hold. That commit queues behind
/// another group's fsync like any write, so a read of the state
/// (`Group::read_local`, what the node's `Node::sys` and every handler call)
/// waits for the disk too. On an async worker that wait stopped the worker,
/// every group's heartbeats and the node's HTTP with it, for as long as the
/// fsync took: CI's one-worker `a_blob_put_waiting_for_the_disk_leaves_the_async_worker_free`
/// ticked 0 times in 300 ms when a system apply was in flight as the disk was
/// held. Told by order: while one held write still waits, and the apply's
/// commit and the read wait behind it, the ticker must keep ticking.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn a_state_read_waiting_for_an_apply_behind_the_disk_leaves_the_worker_free() {
    const GROUP: GroupId = 3;
    const WATCH: Duration = Duration::from_millis(300);
    const MIN_TICKS: u64 = 10;
    let net = Arc::new(Net::default());
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(
        1,
        dir.path(),
        &KEY,
        Arc::new(Wire(Arc::clone(&net))),
        Config::default(),
    )
    .unwrap();
    let group = engine.start_group(GROUP, OnDisk::default()).await.unwrap();
    let _prev = net.engines.write().unwrap().insert(1, Arc::clone(&engine));
    group
        .initialize(BTreeMap::from([(1, "node-1".to_owned())]))
        .await
        .unwrap();
    let propose = |g: Arc<Group<OnDisk>>, key: &'static str| async move {
        for _ in 0..200 {
            match g
                .propose(|_m, _applied| Proposal::Append(key.as_bytes().to_vec()))
                .await
            {
                Ok(p) => return p.index,
                Err(WriteError::NotLeader { .. } | WriteError::Unavailable(_)) => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Err(e) => panic!("{e}"),
            }
        }
        panic!("write never succeeded");
    };
    let warm = propose(Arc::clone(&group), "warm").await;
    assert!(group.wait_applied(warm, HOLD).await);

    // An entry commits but does not apply yet.
    testing::hold_apply(dir.path(), GROUP);
    let appended = group.status().last_log.unwrap_or(0);
    let write = tokio::spawn(propose(Arc::clone(&group), "applied-behind-the-disk"));
    let at = Instant::now();
    while group.status().last_log.unwrap_or(0) <= appended {
        assert!(at.elapsed() < HOLD, "the write never appended");
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    // The disk is held; the apply then takes the state's lock and its commit
    // queues behind the held write.
    testing::slow_synced_writes(dir.path(), NODE_ROWS, HOLD);
    let holder = {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || engine.put_node_meta("slow-disk-test", &1_u8))
    };
    let at = Instant::now();
    let held = loop {
        if let Some(n) = testing::synced_write_waiting(dir.path(), NODE_ROWS) {
            break n;
        }
        assert!(at.elapsed() < HOLD, "the node row's write never waited");
        std::thread::sleep(Duration::from_millis(1));
    };
    testing::release_apply(dir.path(), GROUP);
    let at = Instant::now();
    while testing::writes_queued(dir.path()) == 0 {
        assert!(
            at.elapsed() < HOLD,
            "the apply's commit never queued behind the held write"
        );
        std::thread::sleep(Duration::from_millis(1));
    }

    let ticking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ticks = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ticker = {
        let (ticking, ticks) = (Arc::clone(&ticking), Arc::clone(&ticks));
        tokio::spawn(async move {
            while ticking.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _n = ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        })
    };
    // On the worker, as a request handler's read is (this test's body runs on
    // the thread that blocks on it, not on a worker).
    let read = {
        let group = Arc::clone(&group);
        tokio::spawn(async move { group.read_local(|m, applied| (m.0, applied)) })
    };
    // This thread sleeps, not the runtime: a worker that blocks would stop the
    // runtime's timers too.
    std::thread::sleep(Duration::from_millis(50));
    let before = ticks.load(std::sync::atomic::Ordering::Relaxed);
    std::thread::sleep(WATCH);
    let during = ticks.load(std::sync::atomic::Ordering::Relaxed) - before;
    let still_held = testing::synced_write_waiting(dir.path(), NODE_ROWS) == Some(held);
    let waited = !read.is_finished();
    eprintln!("the worker ticked {during} times in {WATCH:?} while a state read waited");

    testing::heal_synced_writes(dir.path(), NODE_ROWS);
    holder.join().unwrap().unwrap();
    let index = write.await.unwrap();
    let (applied_count, applied_at) = read.await.unwrap();
    ticking.store(false, std::sync::atomic::Ordering::Relaxed);
    ticker.await.unwrap();

    assert!(still_held, "the held write ended before the test looked");
    assert!(
        waited,
        "the read did not wait for the apply behind the held disk"
    );
    assert!(
        during >= MIN_TICKS,
        "the async worker ticked {during} times in {WATCH:?} while a state read waited for an \
         apply behind the disk (at least {MIN_TICKS}): the read blocked it"
    );
    assert_eq!(
        (applied_count, applied_at),
        (2, index),
        "the read sees the apply it waited for, never the state before its commit"
    );
}

/// A one-node group of an [`OnDisk`] machine on a new engine at `dir`, with
/// one entry applied.
async fn one_node(
    dir: &std::path::Path,
    group: GroupId,
) -> (Arc<Net>, Arc<Engine>, Arc<Group<OnDisk>>) {
    let net = Arc::new(Net::default());
    let engine = Engine::open(
        1,
        dir,
        &KEY,
        Arc::new(Wire(Arc::clone(&net))),
        Config::default(),
    )
    .unwrap();
    let g = engine.start_group(group, OnDisk::default()).await.unwrap();
    let _prev = net.engines.write().unwrap().insert(1, Arc::clone(&engine));
    g.initialize(BTreeMap::from([(1, "node-1".to_owned())]))
        .await
        .unwrap();
    for _ in 0..200 {
        match g
            .propose(|_m, _applied| Proposal::Append(b"warm".to_vec()))
            .await
        {
            Ok(p) => {
                assert!(g.wait_applied(p.index, HOLD).await);
                return (net, engine, g);
            }
            Err(WriteError::NotLeader { .. } | WriteError::Unavailable(_)) => {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => panic!("{e}"),
        }
    }
    panic!("write never succeeded");
}

/// Hold the disk of the node at `dir`: a synced write of the node's own rows
/// waits, holding the write path, until [`testing::heal_synced_writes`]. The
/// thread that made it, and the number the held write took.
fn hold_disk(
    engine: &Arc<Engine>,
    dir: &std::path::Path,
) -> (std::thread::JoinHandle<eyre::Result<()>>, u64) {
    testing::slow_synced_writes(dir, NODE_ROWS, HOLD);
    let holder = {
        let engine = Arc::clone(engine);
        std::thread::spawn(move || engine.put_node_meta("slow-disk-test", &1_u8))
    };
    let at = Instant::now();
    loop {
        if let Some(n) = testing::synced_write_waiting(dir, NODE_ROWS) {
            return (holder, n);
        }
        assert!(at.elapsed() < HOLD, "the node row's write never waited");
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Wait (on this thread) until a write of the node at `dir` queues behind the
/// held one.
fn until_queued(dir: &std::path::Path, what: &str) {
    let at = Instant::now();
    while testing::writes_queued(dir) == 0 {
        assert!(
            at.elapsed() < HOLD,
            "{what} never queued behind the held write"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Removing a group deletes its rows, a write like any other: it queues behind
/// another write's fsync. `Engine::remove_group` deleted them on the async
/// worker that awaited it (the node's reconcile, which stops the groups this
/// node no longer replicates), so that worker, every group's heartbeats and the
/// node's HTTP with it, stopped for as long as the disk took. Told by order:
/// while one held write still waits, and the removal waits behind it, the
/// ticker must keep ticking. The removal then completes, a retry of it is a
/// no-op, and the group starts again with nothing of the old state.
#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn removing_a_group_behind_the_disk_leaves_the_worker_free() {
    const GROUP: GroupId = 4;
    const WATCH: Duration = Duration::from_millis(300);
    const MIN_TICKS: u64 = 10;
    let dir = tempfile::tempdir().unwrap();
    let (_net, engine, group) = one_node(dir.path(), GROUP).await;
    assert_eq!(group.read_local(|m, _| m.0), 1);

    let (holder, held) = hold_disk(&engine, dir.path());
    let ticking = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let ticks = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let ticker = {
        let (ticking, ticks) = (Arc::clone(&ticking), Arc::clone(&ticks));
        tokio::spawn(async move {
            while ticking.load(std::sync::atomic::Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
                let _n = ticks.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
        })
    };
    // On the worker, as the node's reconcile awaits it.
    let removal = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.remove_group(GROUP).await })
    };
    until_queued(dir.path(), "the removal's delete");
    // This thread sleeps, not the runtime: a worker that blocks would stop the
    // runtime's timers too.
    std::thread::sleep(Duration::from_millis(50));
    let before = ticks.load(std::sync::atomic::Ordering::Relaxed);
    std::thread::sleep(WATCH);
    let during = ticks.load(std::sync::atomic::Ordering::Relaxed) - before;
    let still_held = testing::synced_write_waiting(dir.path(), NODE_ROWS) == Some(held);
    let waited = !removal.is_finished();
    eprintln!("the worker ticked {during} times in {WATCH:?} while a group's removal waited");

    testing::heal_synced_writes(dir.path(), NODE_ROWS);
    holder.join().unwrap().unwrap();
    removal.await.unwrap().unwrap();
    ticking.store(false, std::sync::atomic::Ordering::Relaxed);
    ticker.await.unwrap();

    assert!(still_held, "the held write ended before the test looked");
    assert!(waited, "the removal did not wait for the held disk");
    assert!(
        during >= MIN_TICKS,
        "the async worker ticked {during} times in {WATCH:?} while a group's removal waited \
         behind the disk (at least {MIN_TICKS}): the removal blocked it"
    );
    assert!(!engine.hosts(GROUP));
    engine.remove_group(GROUP).await.unwrap();
    drop(group);
    let again = engine.start_group(GROUP, OnDisk::default()).await.unwrap();
    assert_eq!(
        again.read_local(|m, applied| (m.0, applied)),
        (0, 0),
        "the removed group's state is gone"
    );
    assert!(
        again.status().last_log.is_none(),
        "the removed group's log is gone"
    );
}

/// While a group's removal waits for the disk, the group is out of the engine
/// but its rows are not yet deleted: a start of the group then (a peer's RPC
/// reaching the group factory) would open a state the removal is deleting
/// under it. It is refused until the removal has finished.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_group_being_removed_does_not_start_again_until_its_data_is_gone() {
    const GROUP: GroupId = 5;
    let dir = tempfile::tempdir().unwrap();
    let (_net, engine, group) = one_node(dir.path(), GROUP).await;
    drop(group);

    let (holder, _held) = hold_disk(&engine, dir.path());
    let removal = {
        let engine = Arc::clone(&engine);
        tokio::spawn(async move { engine.remove_group(GROUP).await })
    };
    until_queued(dir.path(), "the removal's delete");
    let during = tokio::time::timeout(HOLD, engine.start_group(GROUP, OnDisk::default())).await;
    let refused = matches!(during, Ok(Err(_)));
    testing::heal_synced_writes(dir.path(), NODE_ROWS);
    holder.join().unwrap().unwrap();
    removal.await.unwrap().unwrap();
    if let Ok(Ok(g)) = during {
        drop(g);
        engine.remove_group(GROUP).await.unwrap();
    }
    assert!(
        refused,
        "the group started again while its removal waited for the disk"
    );

    let again = engine.start_group(GROUP, OnDisk::default()).await.unwrap();
    assert_eq!(again.read_local(|m, applied| (m.0, applied)), (0, 0));
}
