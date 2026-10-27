//! Three engines in one process, wired by an in-memory transport.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use calimero_log::store::Db;
use calimero_log::types::LogId;
use calimero_log::{
    BoxFuture, Config, Engine, Group, GroupId, GroupState, Machine, NodeId, Proposal, RpcRequest,
    RpcResponse, StateTx, Transport, WriteError,
};

/// A key-value machine: command `key=value` sets, output is the previous value.
#[derive(Default)]
struct Kv {
    map: BTreeMap<String, String>,
}

impl Machine for Kv {
    const IN_MEMORY: bool = true;

    fn apply(&mut self, _index: u64, command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
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
    /// Nodes whose requests arrive but whose answers are lost.
    deaf: Mutex<BTreeSet<NodeId>>,
}

struct Wire(Arc<Net>, NodeId);

impl Transport for Wire {
    fn send(
        &self,
        target: NodeId,
        _addr: String,
        group: GroupId,
        request: RpcRequest,
    ) -> BoxFuture<Result<RpcResponse, String>> {
        let net = Arc::clone(&self.0);
        let me = self.1;
        Box::pin(async move {
            let down = net.down.lock().unwrap().clone();
            if down.contains(&target) || down.contains(&me) {
                return Err("down".into());
            }
            let engine = net.engines.read().unwrap().get(&target).cloned();
            let answer = match engine {
                Some(e) => e.handle_rpc(group, request).await,
                None => return Err("no such node".into()),
            };
            if net.deaf.lock().unwrap().contains(&me) {
                return Err("answer lost".into());
            }
            Ok(answer)
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

async fn start_node(
    net: &Arc<Net>,
    id: NodeId,
    dir: &tempfile::TempDir,
) -> (Arc<Engine>, Arc<Group<Kv>>) {
    let engine = Engine::open(
        id,
        dir.path(),
        &KEY,
        Arc::new(Wire(Arc::clone(net), id)),
        config(),
    )
    .unwrap();
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
        match c.groups[&id]
            .propose(|_m, _applied| Proposal::Append(cmd.clone()))
            .await
        {
            Ok(p) => return p.index,
            Err(WriteError::NotLeader { .. } | WriteError::Unavailable(_)) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            Err(e @ WriteError::TooLarge { .. }) => panic!("{e}"),
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
    assert!(
        c.groups[&old]
            .wait_applied(index, Duration::from_secs(10))
            .await
    );
    assert_eq!(get(&c.groups[&old], "b").as_deref(), Some("2"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn strong_read_needs_the_leader() {
    let c = cluster(3).await;
    let _ = put(&c, "k", "v").await;
    let id = leader(&c).await;
    let v = c.groups[&id]
        .read_strong(|m, _| m.map.get("k").cloned())
        .await
        .unwrap();
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
    let reply = g
        .propose(|_, _| Proposal::Reply(b"cached".to_vec()))
        .await
        .unwrap();
    assert!(!reply.appended);
    assert_eq!(reply.output, b"cached");
}

/// A counter that refuses, at apply, a value built on other state than the
/// one it applies to, as the cloud node's `AppMachine` does with its
/// `base_index`: `set:{base}:{value}` sets only when the last entry it applied
/// is `base`; `add:{n}` adds unconditionally (a write built on no state). Like
/// `AppMachine`, a build names the machine's own last entry, not the log's
/// applied index, which counts blank and membership entries the machine
/// never sees.
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Ledger {
    value: u64,
    /// The index of the last entry applied.
    last: u64,
    /// How many `set`s were refused.
    conflicts: u64,
}

impl Machine for Ledger {
    const IN_MEMORY: bool = true;

    fn apply(&mut self, index: u64, command: &[u8], _state: &mut StateTx<'_>) -> Vec<u8> {
        let s = String::from_utf8_lossy(command).into_owned();
        let parts: Vec<u64> = s.split(':').skip(1).map(|p| p.parse().unwrap()).collect();
        let out: &[u8] = match (s.split(':').next(), parts.as_slice()) {
            (Some("set"), [base, value]) if *base == self.last => {
                self.value = *value;
                b"ok"
            }
            (Some("set"), _) => {
                self.conflicts += 1;
                b"conflict"
            }
            (Some("add"), [n]) => {
                self.value += n;
                b"ok"
            }
            _ => panic!("unknown command {s}"),
        };
        self.last = index;
        out.to_vec()
    }

    fn snapshot(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap()
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        *self = serde_json::from_slice(data)?;
        Ok(())
    }
}

/// Increment the counter on whichever node leads, as `Node::call` runs a
/// call: built on the applied state, built again after a conflict.
async fn increment(groups: &HashMap<NodeId, Arc<Group<Ledger>>>, down: &BTreeSet<NodeId>) {
    for _ in 0..100 {
        let Some(g) = groups
            .values()
            .find(|g| g.status().is_leader && !down.contains(&g.node_id()))
        else {
            tokio::time::sleep(Duration::from_millis(50)).await;
            continue;
        };
        let built = g
            .propose(|m, _| {
                Proposal::Append(format!("set:{}:{}", m.last, m.value + 1).into_bytes())
            })
            .await;
        match built {
            Ok(p) if p.output == b"ok" => return,
            _ => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
    panic!("the increment never applied");
}

async fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..200 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("timed out waiting for {what}");
}

/// `propose` does not confirm leadership before an append. A leader that has
/// lost its majority still builds, on state that a write it could not commit
/// has already made stale, and its entry then commits under the next leader.
/// The machine's base check turns that entry into a no-op: no diff built on
/// stale state is ever applied, on any node.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stale_build_by_a_deposed_leader_never_applies() {
    let c = cluster(3).await;
    let mut groups: HashMap<NodeId, Arc<Group<Ledger>>> = HashMap::new();
    let engines: Vec<(NodeId, Arc<Engine>)> = c
        .net
        .engines
        .read()
        .unwrap()
        .iter()
        .map(|(id, e)| (*id, Arc::clone(e)))
        .collect();
    for (id, engine) in engines {
        let g = engine.start_group(2, Ledger::default()).await.unwrap();
        let _prev = groups.insert(id, g);
    }
    let voters: BTreeMap<NodeId, String> = (1..=3).map(|i| (i, format!("node-{i}"))).collect();
    groups[&1].initialize(voters).await.unwrap();

    increment(&groups, &BTreeSet::new()).await;
    let base = groups
        .values()
        .map(|g| g.status().last_log)
        .max()
        .unwrap()
        .unwrap();
    for g in groups.values() {
        assert!(g.wait_applied(base, Duration::from_secs(5)).await);
    }
    let old = groups
        .values()
        .find(|g| g.status().is_leader)
        .unwrap()
        .node_id();
    let (keeps, cut) = {
        let others: Vec<NodeId> = (1..=3).filter(|i| *i != old).collect();
        (others[0], others[1])
    };
    let conflicts_before = groups[&keeps].read_local(|m, _| m.conflicts);

    // The leader loses its majority: `cut` is gone, and `keeps` receives the
    // leader's entries but the leader never hears back.
    let _ = c.net.down.lock().unwrap().insert(cut);
    let _ = c.net.deaf.lock().unwrap().insert(old);
    let leader = Arc::clone(&groups[&old]);
    let unbuilt = tokio::spawn(async move { leader.write(b"add:5".to_vec()).await });
    wait_for("the write to reach the follower", || {
        groups[&keeps].status().last_log >= Some(base + 1)
    })
    .await;

    // Built on the counter before `add:5`, which the leader has not applied.
    let leader = Arc::clone(&groups[&old]);
    let built = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let saw = Arc::clone(&built);
    let stale = tokio::spawn(async move {
        leader
            .propose(|m, _| {
                saw.store(true, std::sync::atomic::Ordering::SeqCst);
                Proposal::Append(format!("set:{}:{}", m.last, m.value + 1).into_bytes())
            })
            .await
    });
    wait_for("the stale entry to reach the follower", || {
        groups[&keeps].status().last_log >= Some(base + 2)
    })
    .await;
    assert!(
        built.load(std::sync::atomic::Ordering::SeqCst),
        "the leader built without a leadership round"
    );
    assert_eq!(groups[&old].applied(), base, "it built on stale state");

    // The old leader goes; `keeps`, holding both entries, is elected and
    // commits them.
    let _ = c.net.down.lock().unwrap().insert(old);
    let _ = c.net.deaf.lock().unwrap().remove(&old);
    let _ = c.net.down.lock().unwrap().remove(&cut);
    for id in [keeps, cut] {
        assert!(
            groups[&id]
                .wait_applied(base + 2, Duration::from_secs(10))
                .await
        );
        let (value, conflicts) = groups[&id].read_local(|m, _| (m.value, m.conflicts));
        assert_eq!(value, 1 + 5, "the stale set did not overwrite add:5");
        assert_eq!(conflicts, conflicts_before + 1, "it applied as a no-op");
    }

    // The old leader comes back and agrees; whatever its proposal answered,
    // it was not a set that applied.
    let _ = c.net.down.lock().unwrap().remove(&old);
    increment(&groups, &BTreeSet::new()).await;
    let last = groups[&keeps].status().last_log.unwrap();
    for g in groups.values() {
        assert!(g.wait_applied(last, Duration::from_secs(10)).await);
        assert_eq!(g.read_local(|m, _| m.value), 7);
    }
    if let Ok(Ok(Ok(p))) = tokio::time::timeout(Duration::from_secs(5), stale).await {
        assert_eq!(p.output, b"conflict");
    }
    let _ = tokio::time::timeout(Duration::from_secs(5), unbuilt).await;
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
    c.groups[&lid]
        .add_learner(4, "node-4".into())
        .await
        .unwrap();
    assert!(g4.wait_applied(last, Duration::from_secs(10)).await);
    assert_eq!(get(&g4, "k49").as_deref(), Some("49"));
    // Swap node 1 out for node 4.
    let voters: BTreeSet<NodeId> = [2, 3, 4].into_iter().collect();
    let lid = leader(&c).await;
    c.groups[&lid].change_voters(voters.clone()).await.unwrap();
    let _ = c.groups.insert(4, Arc::clone(&g4));
    // The new node learns the membership a moment after the leader commits it.
    let mut seen = BTreeSet::new();
    for _ in 0..50 {
        seen = g4.status().voters.iter().copied().collect();
        if seen == voters {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(seen, voters);
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
        let hay = all_bytes(dir.path());
        assert!(!hay.contains("plaintext-secret-value"));
        assert!(!hay.contains("needle"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn oversized_entries_are_refused_and_blobs_stay_sealed() {
    let c = cluster(3).await;
    let id = leader(&c).await;
    let big = vec![b'x'; calimero_log::MAX_ENTRY_BYTES + 1];
    assert!(matches!(
        c.groups[&id].write(big).await,
        Err(WriteError::TooLarge { .. })
    ));
    // Replication is unaffected.
    let index = put(&c, "after", "1").await;
    for g in c.groups.values() {
        assert!(g.wait_applied(index, Duration::from_secs(5)).await);
    }

    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::open(
        9,
        dir.path(),
        &KEY,
        Arc::new(Wire(Arc::clone(&c.net), 9)),
        config(),
    )
    .unwrap();
    engine.put_blob("b1", b"blob-plaintext-marker").unwrap();
    assert_eq!(
        engine.get_blob("b1").unwrap().as_deref(),
        Some(&b"blob-plaintext-marker"[..])
    );
    assert!(engine.get_blob("b2").unwrap().is_none());
    drop(engine);
    assert!(!all_bytes(dir.path()).contains("blob-plaintext-marker"));
}

/// Every file under `dir`, as one lossy string.
fn all_bytes(dir: &std::path::Path) -> String {
    let mut out = String::new();
    for entry in std::fs::read_dir(dir).unwrap().filter_map(Result::ok) {
        let path = entry.path();
        if path.is_dir() {
            out.push_str(&all_bytes(&path));
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    out
}

/// A key-value machine whose state is the group's durable state: it holds
/// nothing in memory, and records the index of every entry it applies.
struct DiskKv {
    state: Option<GroupState>,
    applies: Arc<Mutex<Vec<u64>>>,
}

impl DiskKv {
    fn get(&self, key: &str) -> Option<String> {
        let v = self.state.as_ref()?.get_hidden(b'k', key.as_bytes())?;
        Some(String::from_utf8(v).unwrap())
    }
}

impl Machine for DiskKv {
    fn attach(&mut self, state: GroupState) {
        self.state = Some(state);
    }

    fn apply(&mut self, index: u64, command: &[u8], state: &mut StateTx<'_>) -> Vec<u8> {
        self.applies.lock().unwrap().push(index);
        let s = String::from_utf8_lossy(command);
        let (k, v) = s.split_once('=').unwrap_or((&s, ""));
        let prev = state.get_hidden(b'k', k.as_bytes()).unwrap_or_default();
        state.put_hidden(b'k', k.as_bytes(), v.as_bytes());
        prev
    }

    fn restore(&mut self, data: &[u8]) -> eyre::Result<()> {
        assert!(
            data.is_empty(),
            "a disk machine restores from its own state"
        );
        Ok(())
    }
}

struct DiskCluster {
    net: Arc<Net>,
    dirs: HashMap<NodeId, tempfile::TempDir>,
    groups: HashMap<NodeId, Arc<Group<DiskKv>>>,
    applies: HashMap<NodeId, Arc<Mutex<Vec<u64>>>>,
}

impl DiskCluster {
    async fn start_node(&mut self, id: NodeId, dir: tempfile::TempDir) -> Arc<Group<DiskKv>> {
        let engine = Engine::open(
            id,
            dir.path(),
            &KEY,
            Arc::new(Wire(Arc::clone(&self.net), id)),
            config(),
        )
        .unwrap();
        let applies = Arc::new(Mutex::new(Vec::new()));
        let group = engine
            .start_group(
                1,
                DiskKv {
                    state: None,
                    applies: Arc::clone(&applies),
                },
            )
            .await
            .unwrap();
        let _prev = self
            .net
            .engines
            .write()
            .unwrap()
            .insert(id, Arc::clone(&engine));
        let _prev = self.dirs.insert(id, dir);
        let _prev = self.groups.insert(id, Arc::clone(&group));
        let _prev = self.applies.insert(id, applies);
        group
    }

    async fn new(n: u64) -> Self {
        let mut c = Self {
            net: Arc::new(Net::default()),
            dirs: HashMap::new(),
            groups: HashMap::new(),
            applies: HashMap::new(),
        };
        for id in 1..=n {
            let _g = c.start_node(id, tempfile::tempdir().unwrap()).await;
        }
        let voters: BTreeMap<NodeId, String> = (1..=n).map(|i| (i, format!("node-{i}"))).collect();
        c.groups[&1].initialize(voters).await.unwrap();
        c
    }

    async fn leader(&self) -> NodeId {
        for _ in 0..100 {
            for g in self.groups.values() {
                if g.status().is_leader {
                    return g.node_id();
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("no leader");
    }

    async fn put(&self, key: &str, value: &str) -> u64 {
        for _ in 0..50 {
            let id = self.leader().await;
            let cmd = format!("{key}={value}").into_bytes();
            match self.groups[&id]
                .propose(|_m, _applied| Proposal::Append(cmd.clone()))
                .await
            {
                Ok(p) => return p.index,
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        panic!("write never succeeded");
    }

    /// Stop a node; its files are handed back.
    async fn stop(&mut self, id: NodeId) -> tempfile::TempDir {
        let engine = self.net.engines.write().unwrap().remove(&id).unwrap();
        engine.shutdown().await;
        drop(engine);
        let _ = self.groups.remove(&id);
        let dir = self.dirs.remove(&id).unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        dir
    }

    async fn restart(&mut self, id: NodeId) -> Arc<Group<DiskKv>> {
        let dir = self.stop(id).await;
        self.start_node(id, dir).await
    }
}

/// A stopped node's stored commit index (group 1).
fn commit_record(dir: &tempfile::TempDir) -> Option<LogId> {
    let db = Db::open(dir.path(), &KEY).unwrap();
    db.get_meta::<Option<LogId>>(1, "committed")
        .unwrap()
        .flatten()
}

/// Replace a stopped node's stored commit index (group 1), as a crash that
/// lost the unsynced write (`None`) or kept an older one would leave it.
fn set_commit_record(dir: &tempfile::TempDir, record: Option<LogId>) {
    let db = Db::open(dir.path(), &KEY).unwrap();
    db.put_meta(1, "committed", &record).unwrap();
}

/// `save_committed` writes the commit index unsynced, on the argument that a
/// node which loses it learns it again. A crash can lose that record or leave
/// an older one: a node restarted so must catch up to the same state and the
/// same commit index, applying every entry once, none twice and none skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lost_or_older_commit_index_is_learned_again() {
    let mut c = DiskCluster::new(3).await;
    let mut keys = Vec::new();
    let mut last = 0;
    for i in 0..10 {
        let key = format!("k{i}");
        last = c.put(&key, &key).await;
        keys.push(key);
    }
    assert!(
        c.groups[&3]
            .wait_applied(last, Duration::from_secs(5))
            .await
    );
    let older = c.groups[&3].raft().metrics().borrow().last_applied.unwrap();
    for i in 10..30 {
        let key = format!("k{i}");
        last = c.put(&key, &key).await;
        keys.push(key);
    }
    // The record rolled back to an older index, then lost.
    for (round, record) in [Some(older), None].into_iter().enumerate() {
        for g in c.groups.values() {
            assert!(g.wait_applied(last, Duration::from_secs(5)).await);
        }
        let dir = c.stop(3).await;
        let stored = commit_record(&dir);
        assert!(
            stored.map(|l| l.index) >= Some(last),
            "round {round}: the store held commit index {stored:?}, behind {last}"
        );
        set_commit_record(&dir, record);
        // The others commit more while it is down.
        let mut expected = Vec::new();
        for i in 0..10 {
            let key = format!("r{round}-{i}");
            last = c.put(&key, &key).await;
            expected.push(last);
            keys.push(key);
        }
        let g = c.start_node(3, dir).await;
        assert!(g.wait_applied(last, Duration::from_secs(10)).await);
        // Each entry it lacked applied once, in order; nothing it had
        // applied before the restart applied again.
        assert_eq!(
            *c.applies[&3].lock().unwrap(),
            expected,
            "round {round}: entries applied after the restart"
        );
        let leader = c.leader().await;
        wait_for("the restarted node to agree with the leader", || {
            let ours = g.raft().metrics().borrow().last_applied;
            ours == c.groups[&leader].raft().metrics().borrow().last_applied
        })
        .await;
        for key in &keys {
            assert_eq!(
                g.read_local(|m, _| m.get(key)).as_deref(),
                Some(key.as_str()),
                "round {round}: {key}"
            );
        }
    }
    // The commit index it learned again reached its store.
    let dir = c.stop(3).await;
    let stored = commit_record(&dir);
    assert!(
        stored.map(|l| l.index) >= Some(last),
        "{stored:?} behind {last}"
    );
}

async fn start_ledger(net: &Arc<Net>, id: NodeId, dir: &tempfile::TempDir) -> Arc<Group<Ledger>> {
    let engine = Engine::open(
        id,
        dir.path(),
        &KEY,
        Arc::new(Wire(Arc::clone(net), id)),
        config(),
    )
    .unwrap();
    let group = engine.start_group(1, Ledger::default()).await.unwrap();
    let _prev = net.engines.write().unwrap().insert(id, engine);
    group
}

/// Append `add:1` on whichever running node leads.
async fn add_one(groups: &HashMap<NodeId, Arc<Group<Ledger>>>) -> u64 {
    for _ in 0..100 {
        if let Some(g) = groups.values().find(|g| g.status().is_leader) {
            if let Ok(p) = g.propose(|_, _| Proposal::Append(b"add:1".to_vec())).await {
                return p.index;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the add never applied");
}

/// The same for an in-memory machine (the system group's kind): it restarts
/// from its last checkpoint and replays its own log up to the commit index
/// it stored, then learns the rest. With that record older than what it had
/// applied, or lost, no entry may apply twice or be skipped; `add` is not
/// idempotent, so either would show in the value.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_in_memory_machine_applies_each_entry_once_after_losing_its_commit_index() {
    let net = Arc::new(Net::default());
    let mut dirs = HashMap::new();
    let mut groups = HashMap::new();
    for id in 1..=3 {
        let dir = tempfile::tempdir().unwrap();
        let _prev = groups.insert(id, start_ledger(&net, id, &dir).await);
        let _prev = dirs.insert(id, dir);
    }
    let voters: BTreeMap<NodeId, String> = (1..=3).map(|i| (i, format!("node-{i}"))).collect();
    groups[&1].initialize(voters).await.unwrap();
    let mut adds = 0;
    let mut last = 0;
    for _ in 0..45 {
        last = add_one(&groups).await;
        adds += 1;
    }
    for round in 0..2 {
        // A follower, so no write is lost with it.
        let leader = loop {
            if let Some(g) = groups.values().find(|g| g.status().is_leader) {
                break g.node_id();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        let id = (1..=3).find(|i| *i != leader).unwrap();
        assert!(groups[&id].wait_applied(last, Duration::from_secs(5)).await);
        // Past its last checkpoint (one per 20 entries), behind what it applies next.
        let older = groups[&id].raft().metrics().borrow().last_applied;
        for _ in 0..5 {
            last = add_one(&groups).await;
            adds += 1;
        }
        assert!(groups[&id].wait_applied(last, Duration::from_secs(5)).await);
        let engine = net.engines.write().unwrap().remove(&id).unwrap();
        engine.shutdown().await;
        drop(engine);
        let _ = groups.remove(&id);
        tokio::time::sleep(Duration::from_millis(200)).await;
        set_commit_record(&dirs[&id], if round == 0 { older } else { None });
        for _ in 0..10 {
            last = add_one(&groups).await;
            adds += 1;
        }
        let _prev = groups.insert(id, start_ledger(&net, id, &dirs[&id]).await);
        for (n, g) in &groups {
            assert!(g.wait_applied(last, Duration::from_secs(10)).await);
            assert_eq!(
                g.read_local(|m, _| m.value),
                adds,
                "round {round}: node {n} applied an entry twice or skipped one"
            );
        }
    }
}

/// Regression for "state machines live in memory": a disk machine resumes
/// where it stopped. Nothing it had applied is applied again on restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_disk_machine_restarts_without_replaying_the_log() {
    let mut c = DiskCluster::new(3).await;
    let mut last = 0;
    for i in 0..30 {
        last = c.put(&format!("k{i}"), &i.to_string()).await;
    }
    for g in c.groups.values() {
        assert!(g.wait_applied(last, Duration::from_secs(5)).await);
    }
    let g = c.restart(3).await;
    assert!(g.wait_applied(last, Duration::from_secs(10)).await);
    // Nothing up to `last` ran again: the state and its applied index were
    // committed together.
    assert_eq!(c.applies[&3].lock().unwrap().len(), 0);
    assert_eq!(g.read_local(|m, _| m.get("k29")).as_deref(), Some("29"));
    assert_eq!(g.read_local(|m, _| m.get("k0")).as_deref(), Some("0"));
    // And it keeps applying.
    let index = c.put("after", "restart").await;
    assert!(g.wait_applied(index, Duration::from_secs(5)).await);
    assert_eq!(
        g.read_local(|m, _| m.get("after")).as_deref(),
        Some("restart")
    );
}

/// A state many snapshot chunks and frames large reaches a new node through a
/// streamed snapshot, and survives that node's restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_big_state_streams_to_a_new_node() {
    let mut c = DiskCluster::new(3).await;
    // ~6 MiB of state: six snapshot chunks, several frames.
    let value = "v".repeat(2048);
    let mut last = 0;
    for i in 0..3000 {
        last = c.put(&format!("key-{i:05}"), &value).await;
    }
    let lid = c.leader().await;
    assert!(
        c.groups[&lid]
            .wait_applied(last, Duration::from_secs(10))
            .await
    );
    // Snapshot and purge, so the new node can only catch up from the snapshot.
    for g in c.groups.values() {
        g.snapshot_now().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let g4 = c.start_node(4, tempfile::tempdir().unwrap()).await;
    let lid = c.leader().await;
    c.groups[&lid]
        .add_learner(4, "node-4".into())
        .await
        .unwrap();
    assert!(g4.wait_applied(last, Duration::from_secs(30)).await);
    // It installed a snapshot rather than replaying 3000 entries.
    assert!(c.applies[&4].lock().unwrap().len() < 3000);
    for i in [0, 1234, 2999] {
        assert_eq!(
            g4.read_local(|m, _| m.get(&format!("key-{i:05}")))
                .as_deref(),
            Some(value.as_str())
        );
    }
    drop(g4);
    let g4 = c.restart(4).await;
    assert_eq!(
        g4.read_local(|m, _| m.get("key-02999")).as_deref(),
        Some(value.as_str())
    );
    // The snapshot on disk is sealed too.
    assert!(!all_bytes(c.dirs[&4].path()).contains(&value[..64]));
}
