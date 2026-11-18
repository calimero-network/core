//! A seeded fault simulator for one group on three engines in one process.
//!
//! Each seed draws a plan of steps (writes, crashes that lose the unsynced
//! writes, restarts, partitions, dropped and delayed messages, snapshot builds
//! and log purges that make a lagging node install a snapshot) and runs it over
//! the in-memory transport, then heals everything and checks:
//!
//! - **no acknowledged write is lost**: every write a leader answered as
//!   committed is on every replica, with its value;
//! - **strong reads see acknowledged writes**: right after a write is
//!   acknowledged, a strong read on its leader sees it;
//! - **replicas agree**: once every replica has applied the same index, their
//!   state digests (`calimero_log::digest`) are equal.
//!
//! `cargo test -p calimero-log --test sim` runs the seeds in `ci_seeds!`, one test each.
//! `SIM_SEED=<n>` runs that seed alone (what a failure prints) in
//! `seeds_from_the_environment`, `SIM_RUNS=<n>` runs `n` more seeds drawn from
//! the clock there, each printed as it starts, and `SIM_TRACE=1` prints the
//! steps of a seed that passes too. The seed fixes the
//! plan and every fault decision, in the order messages are sent; the threads
//! and openraft's own election timeouts are not under its control, so a replay
//! runs the same faults, and may need a few runs to hit the same interleaving.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use calimero_log::store::Db;
use calimero_log::{
    BoxFuture, Comparison, Config, Digests, Engine, Group, GroupId, GroupState, Machine, NodeId,
    Proposal, RpcRequest, RpcResponse, StateTx, Transport,
};

/// Steps per seed: each seed runs in a few seconds, within CI's 15 s a test.
const STEPS: usize = 30;
/// Nodes in the group.
const NODES: u64 = 3;
const KEY: [u8; 32] = [9; 32];
/// How long a write (or the strong read after it) may take before the plan goes
/// on without it: a write with no majority to reach waits for nothing.
const WRITE_WAIT: Duration = Duration::from_secs(1);
const GROUP: GroupId = 1;

/// splitmix64: a small, seeded, reproducible generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number in `0..n`.
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    /// `true` `per_mille` times in a thousand.
    fn chance(&mut self, per_mille: u64) -> bool {
        self.below(1000) < per_mille
    }
}

/// A key-value machine kept in the group's rows.
struct Kv {
    state: Option<GroupState>,
}

impl Kv {
    fn get(&self, key: &str) -> Option<String> {
        let v = self.state.as_ref()?.get_hidden(b'k', key.as_bytes())?;
        Some(String::from_utf8(v).unwrap())
    }
}

impl Machine for Kv {
    fn attach(&mut self, state: GroupState) {
        self.state = Some(state);
    }

    fn apply(&mut self, _index: u64, command: &[u8], state: &mut StateTx<'_>) -> Vec<u8> {
        let s = String::from_utf8_lossy(command);
        let (k, v) = s.split_once('=').unwrap_or((&s, ""));
        state.put_hidden(b'k', k.as_bytes(), v.as_bytes());
        Vec::new()
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        Ok(())
    }
}

/// The faults the network applies now.
#[derive(Default)]
struct Faults {
    /// Links cut both ways.
    cut: BTreeSet<(NodeId, NodeId)>,
    /// Nodes stopped: nothing reaches them.
    down: BTreeSet<NodeId>,
    /// Requests (or their answers) lost, per thousand.
    drop_per_mille: u64,
    /// The most a message waits, in milliseconds.
    delay_max_ms: u64,
}

struct Net {
    engines: RwLock<HashMap<NodeId, Arc<Engine>>>,
    faults: Mutex<Faults>,
    /// Every fault decision, drawn in the order messages are sent.
    rng: Mutex<Rng>,
}

impl Net {
    fn cut(&self, a: NodeId, b: NodeId) -> bool {
        let f = self.faults.lock().unwrap();
        f.cut.contains(&(a.min(b), a.max(b))) || f.down.contains(&a) || f.down.contains(&b)
    }

    /// Draw a message's fate: (lose the request, lose the answer, delay).
    fn fate(&self) -> (bool, bool, Duration) {
        let (per_mille, delay_max) = {
            let f = self.faults.lock().unwrap();
            (f.drop_per_mille, f.delay_max_ms)
        };
        let mut r = self.rng.lock().unwrap();
        let lose_request = r.chance(per_mille);
        let lose_answer = r.chance(per_mille);
        let delay = Duration::from_millis(r.below(delay_max + 1));
        (lose_request, lose_answer, delay)
    }
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
            if net.cut(me, target) {
                return Err("cut".into());
            }
            let (lose_request, lose_answer, delay) = net.fate();
            tokio::time::sleep(delay).await;
            if lose_request || net.cut(me, target) {
                return Err("lost".into());
            }
            let engine = net.engines.read().unwrap().get(&target).cloned();
            let Some(engine) = engine else {
                return Err("no such node".into());
            };
            let answer = engine.handle_rpc(group, request).await;
            if lose_answer || net.cut(me, target) {
                return Err("answer lost".into());
            }
            Ok(answer)
        })
    }
}

fn config() -> Config {
    Config {
        heartbeat_ms: 50,
        election_min_ms: 300,
        election_max_ms: 600,
        snapshot_every: 15,
    }
}

struct Sim {
    seed: u64,
    net: Arc<Net>,
    dirs: HashMap<NodeId, tempfile::TempDir>,
    groups: HashMap<NodeId, Arc<Group<Kv>>>,
    /// Stopped nodes' directories.
    stopped: HashMap<NodeId, tempfile::TempDir>,
    /// Writes a leader acknowledged: key -> value.
    acked: BTreeMap<String, String>,
    /// What happened, for the report of a failing seed.
    trace: String,
    began: std::time::Instant,
}

impl Sim {
    async fn start(seed: u64) -> Self {
        let net = Arc::new(Net {
            engines: RwLock::new(HashMap::new()),
            faults: Mutex::new(Faults::default()),
            rng: Mutex::new(Rng(seed ^ 0x5EED)),
        });
        let mut s = Self {
            seed,
            net,
            dirs: HashMap::new(),
            groups: HashMap::new(),
            stopped: HashMap::new(),
            acked: BTreeMap::new(),
            trace: String::new(),
            began: std::time::Instant::now(),
        };
        for id in 1..=NODES {
            s.run_node(id, tempfile::tempdir().unwrap()).await;
        }
        let voters: BTreeMap<NodeId, String> =
            (1..=NODES).map(|i| (i, format!("node-{i}"))).collect();
        s.groups[&1].initialize(voters).await.unwrap();
        s
    }

    fn note(&mut self, step: usize, what: impl std::fmt::Display) {
        let at = self.began.elapsed().as_millis();
        let _w = writeln!(self.trace, "  {step:>3} @{at:>6} ms: {what}");
    }

    async fn run_node(&mut self, id: NodeId, dir: tempfile::TempDir) {
        let engine = Engine::open(
            id,
            dir.path(),
            &KEY,
            Arc::new(Wire(Arc::clone(&self.net), id)),
            config(),
        )
        .unwrap();
        let group = engine.start_group(GROUP, Kv { state: None }).await.unwrap();
        let _prev = self
            .net
            .engines
            .write()
            .unwrap()
            .insert(id, Arc::clone(&engine));
        let _prev = self.groups.insert(id, group);
        let _prev = self.dirs.insert(id, dir);
        let _was = self.net.faults.lock().unwrap().down.remove(&id);
    }

    /// Stop node `id`, losing its unsynced writes when `lose` (a crash before
    /// they reached the disk).
    async fn crash(&mut self, id: NodeId, lose: bool) {
        let _new = self.net.faults.lock().unwrap().down.insert(id);
        let engine = self.net.engines.write().unwrap().remove(&id).unwrap();
        if lose {
            engine.lose_unsynced_writes(GROUP);
        }
        engine.shutdown().await;
        drop(engine);
        let _ = self.groups.remove(&id);
        let dir = self.dirs.remove(&id).unwrap();
        // Its store is free once RocksDB lets go of the lock.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while Db::open(dir.path(), &KEY).is_err() {
            assert!(
                std::time::Instant::now() < deadline,
                "node {id}'s store stays held"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _prev = self.stopped.insert(id, dir);
    }

    async fn restart(&mut self, id: NodeId) {
        let dir = self.stopped.remove(&id).unwrap();
        self.run_node(id, dir).await;
    }

    fn leader(&self) -> Option<NodeId> {
        self.groups
            .values()
            .find(|g| g.status().is_leader)
            .map(|g| g.node_id())
    }

    /// One write through whoever leads; acknowledged writes are kept. A strong
    /// read on that leader must then see it.
    async fn write(&mut self, step: usize, key: String, value: String) {
        let Some(id) = self.leader() else {
            self.note(step, format!("write {key}: no leader"));
            return;
        };
        let g = Arc::clone(&self.groups[&id]);
        let cmd = format!("{key}={value}").into_bytes();
        let wrote = tokio::time::timeout(
            WRITE_WAIT,
            g.propose(|_m, _applied| Proposal::Append(cmd.clone())),
        )
        .await;
        match wrote {
            Ok(Ok(p)) if p.appended => {
                let _prev = self.acked.insert(key.clone(), value.clone());
                self.note(step, format!("write {key} on node {id}: index {}", p.index));
                let read =
                    tokio::time::timeout(WRITE_WAIT, g.read_strong(|m, _| m.get(&key))).await;
                if let Ok(Ok(seen)) = read {
                    assert_eq!(
                        seen.as_deref(),
                        Some(value.as_str()),
                        "seed {}: a strong read on node {id} right after write {key} \
                         missed it\n{}",
                        self.seed,
                        self.trace
                    );
                }
            }
            other => {
                let why = match other {
                    Ok(Ok(_)) => "not appended".to_owned(),
                    Ok(Err(e)) => e.to_string(),
                    Err(_) => "timed out".to_owned(),
                };
                self.note(step, format!("write {key} on node {id}: {why}"));
            }
        }
    }

    /// Build a snapshot on `id` and purge its log behind it, so a node that
    /// lags it has to install the snapshot.
    async fn snapshot_and_purge(&mut self, step: usize, id: NodeId) {
        let g = Arc::clone(&self.groups[&id]);
        let applied = g.applied();
        let built = g.snapshot_through(applied, Duration::from_secs(2)).await;
        let purged = match built {
            Ok(()) => g.purge_log_through(applied, Duration::from_secs(2)).await,
            Err(e) => Err(e),
        };
        self.note(
            step,
            format!("snapshot and purge on node {id} through {applied}: {purged:?}"),
        );
    }

    async fn run_plan(&mut self, rng: &mut Rng) {
        for step in 0..STEPS {
            match rng.below(100) {
                0..=54 => {
                    let key = format!("k{step}");
                    let value = format!("v{}", rng.below(1_000_000));
                    self.write(step, key, value).await;
                }
                55..=64 => {
                    // Crash a running node, if that leaves a majority running
                    // and reachable (no partition on top of it).
                    let running: Vec<NodeId> = self.groups.keys().copied().collect();
                    let partitioned = !self.net.faults.lock().unwrap().cut.is_empty();
                    if running.len() as u64 > NODES / 2 + 1 && !partitioned {
                        let id = running[rng.below(running.len() as u64) as usize];
                        let lose = rng.chance(500);
                        self.note(
                            step,
                            format!("crash node {id}, losing unsynced writes: {lose}"),
                        );
                        self.crash(id, lose).await;
                    }
                }
                65..=74 => {
                    let stopped: Vec<NodeId> = self.stopped.keys().copied().collect();
                    if !stopped.is_empty() {
                        let id = stopped[rng.below(stopped.len() as u64) as usize];
                        self.note(step, format!("restart node {id}"));
                        self.restart(id).await;
                    }
                }
                75..=82 => {
                    let id = 1 + rng.below(NODES);
                    let mut f = self.net.faults.lock().unwrap();
                    f.cut.clear();
                    for other in 1..=NODES {
                        if other != id {
                            let _new = f.cut.insert((id.min(other), id.max(other)));
                        }
                    }
                    drop(f);
                    self.note(step, format!("partition node {id} off"));
                }
                83..=87 => {
                    self.net.faults.lock().unwrap().cut.clear();
                    self.note(step, "heal partitions");
                }
                88..=93 => {
                    let drop = [0, 20, 100][rng.below(3) as usize];
                    let delay = [0, 20, 80][rng.below(3) as usize];
                    {
                        let mut f = self.net.faults.lock().unwrap();
                        f.drop_per_mille = drop;
                        f.delay_max_ms = delay;
                    }
                    self.note(
                        step,
                        format!("messages: drop {drop}‰, delay up to {delay} ms"),
                    );
                }
                _ => {
                    let running: Vec<NodeId> = self.groups.keys().copied().collect();
                    let id = running[rng.below(running.len() as u64) as usize];
                    self.snapshot_and_purge(step, id).await;
                }
            }
            tokio::time::sleep(Duration::from_millis(rng.below(150))).await;
        }
    }

    /// Heal everything, bring every node back, and wait until every replica has
    /// applied the same index.
    async fn settle(&mut self) -> u64 {
        {
            let mut f = self.net.faults.lock().unwrap();
            f.cut.clear();
            f.drop_per_mille = 0;
            f.delay_max_ms = 0;
        }
        let stopped: Vec<NodeId> = self.stopped.keys().copied().collect();
        for id in stopped {
            self.restart(id).await;
        }
        // One more write commits through the current term, so every earlier
        // entry is committed and applied wherever it is applied.
        let mut last = 0;
        for _ in 0..100 {
            if let Some(id) = self.leader() {
                let g = Arc::clone(&self.groups[&id]);
                if let Ok(p) = g
                    .propose(|_m, _| Proposal::Append(b"settled=yes".to_vec()))
                    .await
                {
                    last = p.index;
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(
            last > 0,
            "seed {}: no write commits once healed\n{}",
            self.seed,
            self.trace
        );
        for (id, g) in &self.groups {
            assert!(
                g.wait_applied(last, Duration::from_secs(20)).await,
                "seed {}: node {id} never applied {last} once healed\n{}",
                self.seed,
                self.trace
            );
        }
        last
    }

    fn check(&self) {
        let seed = self.seed;
        for (id, g) in &self.groups {
            for (key, value) in &self.acked {
                assert_eq!(
                    g.read_local(|m, _| m.get(key)).as_deref(),
                    Some(value.as_str()),
                    "seed {seed}: node {id} lost acknowledged write {key}\n{}",
                    self.trace
                );
            }
        }
    }

    /// Every replica's digests once all have applied the same index (an
    /// election after `last` may add a blank entry some have not applied yet).
    async fn digests_at_one_index(&self, last: u64) -> BTreeMap<NodeId, Digests> {
        let mut digests = BTreeMap::new();
        for _ in 0..200 {
            digests = self
                .groups
                .iter()
                .map(|(id, g)| (*id, g.digests().unwrap()))
                .collect();
            let indexes: BTreeSet<u64> = digests.values().map(|d| d.applied.index).collect();
            if indexes.len() == 1 && indexes.iter().all(|i| *i >= last) {
                return digests;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!(
            "seed {}: the replicas never stood at one index: {:?}\n{}",
            self.seed,
            digests
                .iter()
                .map(|(id, d)| (id, d.applied.index))
                .collect::<Vec<_>>(),
            self.trace
        );
    }

    async fn check_digests(&self, last: u64) {
        let seed = self.seed;
        let digests = self.digests_at_one_index(last).await;
        let (first, ours) = digests.iter().next().unwrap();
        for (id, theirs) in &digests {
            assert!(
                matches!(ours.compare(theirs), Comparison::Agree { .. }),
                "seed {seed}: nodes {first} and {id} hold different states: {:?}\n{}",
                ours.compare(theirs),
                self.trace
            );
        }
    }

    async fn stop(mut self) {
        let ids: Vec<NodeId> = self.groups.keys().copied().collect();
        for id in ids {
            self.crash(id, false).await;
        }
    }
}

async fn run_seed(seed: u64) {
    eprintln!("sim: seed {seed} (replay with SIM_SEED={seed})");
    let mut rng = Rng(seed);
    let mut sim = Sim::start(seed).await;
    sim.run_plan(&mut rng).await;
    let last = sim.settle().await;
    sim.check();
    sim.check_digests(last).await;
    if std::env::var_os("SIM_TRACE").is_some() {
        eprintln!("{}", sim.trace);
    }
    sim.stop().await;
}

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.parse().ok()
}

/// One test per seed CI runs, so each stays within CI's per-test time.
macro_rules! ci_seeds {
    ($($name:ident = $seed:literal),* $(,)?) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $name() {
                run_seed($seed).await;
            }
        )*
    };
}

ci_seeds!(
    seed_1 = 1,
    seed_2 = 2,
    seed_3 = 3,
    seed_4 = 4,
    seed_8609365135234708664 = 8609365135234708664
);

/// `SIM_SEED=<n>` replays one seed; `SIM_RUNS=<n>` runs `n` more, drawn from the
/// clock. Without either, nothing (the CI seeds have their own tests).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn seeds_from_the_environment() {
    if let Some(seed) = env_u64("SIM_SEED") {
        run_seed(seed).await;
        return;
    }
    let more = env_u64("SIM_RUNS").unwrap_or(0);
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    let mut seeds = Rng(start);
    for _ in 0..more {
        run_seed(seeds.next()).await;
    }
}
