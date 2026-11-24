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
