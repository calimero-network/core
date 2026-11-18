//! A slow disk under one group does not hold up the node's other groups.
//!
//! Every group of a node shares the node's async workers. A synced write (a log
//! append, a vote) or snapshot file I/O run inline on one would hold it for as
//! long as the disk takes, and every other group's heartbeats, elections and
//! RPCs wait behind it. The runtime here has one worker, so any blocking call
//! shows as the whole of its delay.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};
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
struct Net(RwLock<HashMap<NodeId, Arc<Engine>>>);

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
            let engine = net.0.read().unwrap().get(&target).cloned();
            match engine {
                Some(e) => Ok(e.handle_rpc(group, request).await),
                None => Err("no such node".into()),
            }
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

async fn put(groups: &Groups, key: &str) {
    for _ in 0..50 {
        let id = leader(groups).await;
        let cmd = format!("{key}=v").into_bytes();
        match groups[&id]
            .propose(|_m, _applied| Proposal::Append(cmd.clone()))
            .await
        {
            Ok(_) => return,
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
/// (and every other group) for the whole fsync.
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
        let _prev = net.0.write().unwrap().insert(id, engine);
        dirs.push(dir);
    }
    let voters: BTreeMap<NodeId, String> = (1..=3).map(|i| (i, format!("node-{i}"))).collect();
    slow[&1].initialize(voters.clone()).await.unwrap();
    healthy[&1].initialize(voters).await.unwrap();
    put(&slow, "warm").await;
    put(&healthy, "warm").await;
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
    let (_, healthy_writes) = tokio::join!(
        async {
            for i in 0..4 {
                put(&slow, &format!("slow{i}")).await;
            }
        },
        async {
            let mut longest = Duration::ZERO;
            while started.elapsed() < SYNC_DELAY * 6 {
                let at = Instant::now();
                put(&healthy, "beside").await;
                longest = longest.max(at.elapsed());
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            longest
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
    assert!(
        healthy_writes < MAX_LATE,
        "a write to the healthy group took {healthy_writes:?} beside the slow one"
    );
    assert_eq!(
        terms(&healthy),
        healthy_terms,
        "the healthy group elected a new leader while the other's disk was slow"
    );
}
