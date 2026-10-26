//! Three engines in one process, wired by an in-memory transport.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use calimero_log::{
    BoxFuture, Config, Engine, Group, GroupId, Machine, NodeId, Proposal, RpcRequest, RpcResponse,
    Transport, WriteError,
};

/// A key-value machine: command `key=value` sets, output is the previous value.
#[derive(Default)]
struct Kv {
    map: BTreeMap<String, String>,
}

impl Machine for Kv {
    fn apply(&mut self, _index: u64, command: &[u8]) -> Vec<u8> {
        let s = String::from_utf8_lossy(command);
        let (k, v) = s.split_once('=').unwrap_or((&s, ""));
        self.map
            .insert(k.to_owned(), v.to_owned())
            .unwrap_or_default()
            .into_bytes()
    }

    fn snapshot(&self) -> Vec<u8> {
        serde_json::to_vec(&self.map).unwrap()
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        self.map = serde_json::from_slice(data)?;
        Ok(())
    }
}

#[derive(Default)]
struct Net {
    engines: RwLock<HashMap<NodeId, Arc<Engine>>>,
    down: Mutex<BTreeSet<NodeId>>,
}

struct Wire(Arc<Net>, NodeId);

impl Transport for Wire {
    fn send(&self, target: NodeId, _addr: String, group: GroupId, request: RpcRequest) -> BoxFuture<Result<RpcResponse, String>> {
        let net = Arc::clone(&self.0);
        let me = self.1;
        Box::pin(async move {
            let down = net.down.lock().unwrap().clone();
            if down.contains(&target) || down.contains(&me) {
                return Err("down".into());
            }
            let engine = net.engines.read().unwrap().get(&target).cloned();
            match engine {
                Some(e) => Ok(e.handle_rpc(group, request).await),
                None => Err("no such node".into()),
            }
        })
    }
}

struct Cluster {
    net: Arc<Net>,
    dirs: Vec<tempfile::TempDir>,
    groups: HashMap<NodeId, Arc<Group<Kv>>>,
}

const KEY: [u8; 32] = [7; 32];

fn config() -> Config {
    Config {
        heartbeat_ms: 50,
        election_min_ms: 300,
        election_max_ms: 600,
        snapshot_every: 20,
    }
}

async fn start_node(net: &Arc<Net>, id: NodeId, dir: &tempfile::TempDir) -> (Arc<Engine>, Arc<Group<Kv>>) {
    let engine = Engine::open(id, &dir.path().join("log.redb"), &KEY, Arc::new(Wire(Arc::clone(net), id)), config()).unwrap();
    let group = engine.start_group(1, Kv::default()).await.unwrap();
    let _prev = net.engines.write().unwrap().insert(id, Arc::clone(&engine));
    (engine, group)
}

async fn cluster(n: u64) -> Cluster {
    let net = Arc::new(Net::default());
    let mut dirs = Vec::new();
    let mut groups = HashMap::new();
    for id in 1..=n {
        let dir = tempfile::tempdir().unwrap();
        let (_e, g) = start_node(&net, id, &dir).await;
        let _prev = groups.insert(id, g);
        dirs.push(dir);
    }
    let voters: BTreeMap<NodeId, String> = (1..=n).map(|i| (i, format!("node-{i}"))).collect();
    groups[&1].initialize(voters).await.unwrap();
    Cluster { net, dirs, groups }
}

async fn leader(c: &Cluster) -> NodeId {
    for _ in 0..100 {
        for g in c.groups.values() {
            let s = g.status();
            if s.is_leader && !c.net.down.lock().unwrap().contains(&g.node_id()) {
                return g.node_id();
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("no leader");
}

async fn put(c: &Cluster, key: &str, value: &str) -> u64 {
    for _ in 0..50 {
        let id = leader(c).await;
        let cmd = format!("{key}={value}").into_bytes();
        match c.groups[&id].propose(|_m, _applied| Proposal::Append(cmd)).await {
            Ok(p) => return p.index,
            Err(WriteError::NotLeader { .. } | WriteError::Unavailable(_)) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
    panic!("write never succeeded");
}

fn get(g: &Group<Kv>, key: &str) -> Option<String> {
    g.read_local(|m, _| m.map.get(key).cloned())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replicates_to_every_node() {
    let c = cluster(3).await;
    let index = put(&c, "a", "1").await;
    for g in c.groups.values() {
        assert!(g.wait_applied(index, Duration::from_secs(5)).await);
        assert_eq!(get(g, "a").as_deref(), Some("1"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn survives_leader_loss() {
    let c = cluster(3).await;
    let _ = put(&c, "a", "1").await;
    let old = leader(&c).await;
    let _inserted = c.net.down.lock().unwrap().insert(old);
    let index = put(&c, "b", "2").await;
    let new = leader(&c).await;
    assert_ne!(old, new);
    for (id, g) in &c.groups {
        if *id == old {
            continue;
        }
        assert!(g.wait_applied(index, Duration::from_secs(5)).await);
        assert_eq!(get(g, "b").as_deref(), Some("2"));
    }
    // The old leader comes back and catches up.
    let _removed = c.net.down.lock().unwrap().remove(&old);
    assert!(c.groups[&old].wait_applied(index, Duration::from_secs(10)).await);
    assert_eq!(get(&c.groups[&old], "b").as_deref(), Some("2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strong_read_needs_the_leader() {
    let c = cluster(3).await;
    let _ = put(&c, "k", "v").await;
    let id = leader(&c).await;
    let v = c.groups[&id].read_strong(|m, _| m.map.get("k").cloned()).await.unwrap();
    assert_eq!(v.as_deref(), Some("v"));
    let follower = c.groups.keys().find(|k| **k != id).unwrap();
    assert!(matches!(
        c.groups[follower].read_strong(|m, _| m.map.len()).await,
        Err(WriteError::NotLeader { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn proposals_see_previous_writes() {
    let c = cluster(3).await;
    let id = leader(&c).await;
    let g = &c.groups[&id];
    // A counter built from current state: each proposal reads then writes.
    for _ in 0..10 {
        let _p = g
            .propose(|m, _| {
                let n: u64 = m.map.get("n").map_or(0, |v| v.parse().unwrap());
                Proposal::Append(format!("n={}", n + 1).into_bytes())
            })
            .await
            .unwrap();
    }
    assert_eq!(get(g, "n").as_deref(), Some("10"));
    let reply = g.propose(|_, _| Proposal::Reply(b"cached".to_vec())).await.unwrap();
    assert!(!reply.appended);
    assert_eq!(reply.output, b"cached");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restarts_from_disk() {
    let mut c = cluster(3).await;
    let mut last = 0;
    for i in 0..30 {
        last = put(&c, &format!("k{i}"), &i.to_string()).await;
    }
    for g in c.groups.values() {
        assert!(g.wait_applied(last, Duration::from_secs(5)).await);
    }
    // Restart node 3 from its files.
    let engine = c.net.engines.write().unwrap().remove(&3).unwrap();
    engine.shutdown().await;
    drop(engine);
    let _ = c.groups.remove(&3);
    let dir = c.dirs.remove(2);
    tokio::time::sleep(Duration::from_millis(200)).await;
    let (_e, g) = start_node(&c.net, 3, &dir).await;
    assert!(g.wait_applied(last, Duration::from_secs(10)).await);
    assert_eq!(get(&g, "k29").as_deref(), Some("29"));
    assert_eq!(get(&g, "k0").as_deref(), Some("0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn new_node_catches_up_and_becomes_voter() {
    let mut c = cluster(3).await;
    let mut last = 0;
    for i in 0..50 {
        last = put(&c, &format!("k{i}"), &i.to_string()).await;
    }
    let dir = tempfile::tempdir().unwrap();
    let (_e, g4) = start_node(&c.net, 4, &dir).await;
    let lid = leader(&c).await;
    c.groups[&lid].add_learner(4, "node-4".into()).await.unwrap();
    assert!(g4.wait_applied(last, Duration::from_secs(10)).await);
    assert_eq!(get(&g4, "k49").as_deref(), Some("49"));
    // Swap node 1 out for node 4.
    let voters: BTreeSet<NodeId> = [2, 3, 4].into_iter().collect();
    let lid = leader(&c).await;
    c.groups[&lid].change_voters(voters.clone()).await.unwrap();
    let _ = c.groups.insert(4, Arc::clone(&g4));
    let s = g4.status();
    assert_eq!(s.voters.iter().copied().collect::<BTreeSet<_>>(), voters);
    c.dirs.push(dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disk_holds_no_plaintext() {
    let c = cluster(3).await;
    let index = put(&c, "needle", "plaintext-secret-value").await;
    for g in c.groups.values() {
        assert!(g.wait_applied(index, Duration::from_secs(5)).await);
    }
    for g in c.groups.values() {
        g.snapshot_now().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    for dir in &c.dirs {
        let bytes = std::fs::read(dir.path().join("log.redb")).unwrap();
        let hay = String::from_utf8_lossy(&bytes);
        assert!(!hay.contains("plaintext-secret-value"));
        assert!(!hay.contains("needle"));
    }
}
