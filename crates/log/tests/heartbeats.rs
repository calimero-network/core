//! Heartbeats batched per peer (B6), through a rollout: old and new nodes in
//! one cluster, batching turned on node by node, and a peer that refuses a
//! batch. No group may lose its leader or move to a new term on the way.

#![allow(
    clippy::unwrap_used,
    reason = "a test's panic is its report; clippy.toml exempts #[test] fns, not their helpers"
)]

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use calimero_log::{
    BoxFuture, Config, Engine, Group, GroupId, Machine, NodeId, RpcRequest, RpcResponse, StateTx,
    Transport,
};

#[derive(Default)]
struct Idle;

impl Machine for Idle {
    const IN_MEMORY: bool = true;

    fn apply(&mut self, _index: u64, _command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
        Vec::new()
    }

    fn snapshot(&self) -> eyre::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Net {
    engines: RwLock<HashMap<NodeId, Arc<Engine>>>,
    /// Nodes on a release before batches: they refuse one as a whole (the
    /// node answers a route it does not have with 404).
    old: Mutex<BTreeSet<NodeId>>,
    /// Batches delivered, and the heartbeats they carried.
    batches: AtomicU64,
    batched: AtomicU64,
    /// Heartbeats (empty appends) sent on their own.
    single: AtomicU64,
}

struct Wire(Arc<Net>);

impl Wire {
    fn engine(&self, target: NodeId) -> Result<Arc<Engine>, String> {
        self.0
            .engines
            .read()
            .unwrap()
            .get(&target)
            .cloned()
            .ok_or_else(|| "no such node".to_owned())
    }
}

impl Transport for Wire {
    fn send(
        &self,
        target: NodeId,
        _addr: String,
        group: GroupId,
        request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>> {
        if matches!(&request, RpcRequest::Append(r) if r.entries.is_empty()) {
            let _n = self.0.single.fetch_add(1, Ordering::Relaxed);
        }
        let engine = self.engine(target);
        Box::pin(async move { Ok(engine?.handle_rpc(group, request).await) })
    }

    fn send_batch(
        &self,
        target: NodeId,
        _addr: String,
        items: Vec<(GroupId, RpcRequest)>,
    ) -> BoxFuture<Result<Vec<Result<RpcResponse, String>>, String>> {
        let net = Arc::clone(&self.0);
        let engine = self.engine(target);
        Box::pin(async move {
            if net.old.lock().unwrap().contains(&target) {
                return Err("404: no such route".to_owned());
            }
            let _n = net.batches.fetch_add(1, Ordering::Relaxed);
            let _n = net.batched.fetch_add(items.len() as u64, Ordering::Relaxed);
            let answers = engine?.handle_rpc_batch(items).await;
            Ok(answers.into_iter().map(Ok).collect())
        })
    }
}

const GROUPS: u64 = 16;
const HEARTBEAT_MS: u64 = 50;
/// Longer than the longest election timeout, three times over.
const HOLD: Duration = Duration::from_millis(1_800);

struct Cluster {
    net: Arc<Net>,
    _dirs: Vec<tempfile::TempDir>,
    engines: BTreeMap<NodeId, Arc<Engine>>,
    /// Every replica of every group.
    replicas: Vec<Arc<Group<Idle>>>,
}

/// Three nodes hosting `GROUPS` groups, half led by node 1 and half by node 2.
async fn cluster() -> Cluster {
    let config = Config {
        heartbeat_ms: HEARTBEAT_MS,
        election_min_ms: 300,
        election_max_ms: 600,
        snapshot_every: 1_000,
    };
    let net = Arc::new(Net::default());
    let (mut dirs, mut engines, mut replicas) = (Vec::new(), BTreeMap::new(), Vec::new());
    let mut by_node: HashMap<(NodeId, GroupId), Arc<Group<Idle>>> = HashMap::new();
    for id in 1..=3 {
        let dir = tempfile::tempdir().unwrap();
        let engine = Engine::open(
            id,
            dir.path(),
            &[7; 32],
            Arc::new(Wire(Arc::clone(&net))),
            config.clone(),
        )
        .unwrap();
        let _prev = net.engines.write().unwrap().insert(id, Arc::clone(&engine));
        for g in 1..=GROUPS {
            let group = engine.start_group(g, Idle).await.unwrap();
            let _prev = by_node.insert((id, g), Arc::clone(&group));
            replicas.push(group);
        }
        let _prev = engines.insert(id, engine);
        dirs.push(dir);
    }
    let voters: BTreeMap<NodeId, String> = (1..=3).map(|i| (i, format!("node-{i}"))).collect();
    for g in 1..=GROUPS {
        let first = if g <= GROUPS / 2 { 1 } else { 2 };
        by_node[&(first, g)]
            .initialize(voters.clone())
            .await
            .unwrap();
    }
    let c = Cluster {
        net,
        _dirs: dirs,
        engines,
        replicas,
    };
    for _ in 0..200 {
        if c.leaders().len() == GROUPS as usize {
            return c;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("not every group elected a leader");
}

impl Cluster {
    /// Each group's leader and term, where one replica leads it and every
    /// replica is in its term.
    fn leaders(&self) -> BTreeMap<GroupId, (NodeId, u64)> {
        let mut by_group: BTreeMap<GroupId, Vec<_>> = BTreeMap::new();
        for r in &self.replicas {
            by_group.entry(r.id()).or_default().push(r.status());
        }
        by_group
            .into_iter()
            .filter_map(|(g, s)| {
                let leader = s.iter().find(|s| s.is_leader)?;
                let id = leader.leader?;
                s.iter()
                    .all(|r| r.term == leader.term && r.leader == Some(id))
                    .then_some((g, (id, leader.term)))
            })
            .collect()
    }

    fn batch(&self, nodes: &[NodeId], on: bool) {
        for n in nodes {
            self.engines[n].set_batch_heartbeats(on);
        }
    }

    fn set_old(&self, node: NodeId, old: bool) {
        let mut set = self.net.old.lock().unwrap();
        let _changed = if old {
            set.insert(node)
        } else {
            set.remove(&node)
        };
    }

    /// Wait `HOLD` and check that every group kept its leader and its term.
    async fn holds(&self, before: &BTreeMap<GroupId, (NodeId, u64)>, phase: &str) {
        tokio::time::sleep(HOLD).await;
        assert_eq!(
            &self.leaders(),
            before,
            "{phase}: a group changed leader or term"
        );
    }

    fn counts(&self) -> (u64, u64, u64) {
        let n = &self.net;
        (
            n.batches.load(Ordering::Relaxed),
            n.batched.load(Ordering::Relaxed),
            n.single.load(Ordering::Relaxed),
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_mixed_cluster_keeps_its_leaders_through_the_batching_rollout() {
    let c = cluster().await;
    let before = c.leaders();
    assert_eq!(before.len(), GROUPS as usize);

    // A cluster below the feature: node 3 runs the release before, no node
    // batches, and each group heartbeats on its own.
    c.set_old(3, true);
    c.holds(&before, "before the rollout").await;
    let (batches, _, single) = c.counts();
    assert_eq!(batches, 0, "a node batched below the cluster version");
    assert!(single > 0);

    // Node 3 is replaced by a new one, and the cluster version reaches the
    // nodes one at a time: node 1 batches while 2 and 3 do not yet; every
    // node takes both forms.
    c.set_old(3, false);
    c.batch(&[1], true);
    c.holds(&before, "node 1 batching").await;
    assert!(c.counts().0 > 0, "node 1 sent no batch");

    c.batch(&[2, 3], true);
    tokio::time::sleep(Duration::from_millis(4 * HEARTBEAT_MS)).await;
    let (b0, h0, s0) = c.counts();
    c.holds(&before, "every node batching").await;
    let (b1, h1, s1) = c.counts();
    let (batches, carried, single) = (b1 - b0, h1 - h0, s1 - s0);
    // Two leaders, each with two followers, each holding half the groups: a
    // tick's heartbeats to one peer are one batch of `GROUPS / 2`.
    assert_eq!(single, 0, "a heartbeat went on its own while batching");
    assert!(
        carried >= batches * (GROUPS / 4),
        "{carried} heartbeats in {batches} batches: not gathered per peer"
    );
    println!("every node batching: {carried} heartbeats in {batches} batches over {HOLD:?}");

    // A peer that does not take a batch (as an old node would, were the gate
    // wrong): each heartbeat is sent again on its own and answered.
    c.set_old(3, true);
    let (_, _, s0) = c.counts();
    c.holds(&before, "a peer refusing batches").await;
    assert!(
        c.counts().2 > s0,
        "no heartbeat fell back to its own message"
    );

    // And back: batching off everywhere is the release before's behaviour.
    c.set_old(3, false);
    c.batch(&[1, 2, 3], false);
    c.holds(&before, "batching off again").await;

    for e in c.engines.values() {
        e.shutdown().await;
    }
}
