//! What a node's Raft traffic costs as it hosts more groups (B6): `n` groups on
//! three engines in one process (the in-memory transport of `tests/cluster.rs`),
//! each led by node 1, idle, on the production timing (`Config::default()`).
//! For 1, 100 and 1,000 groups (`HEARTBEAT_GROUPS=1,100,1000`) it prints the
//! network messages node 1 sends one follower per second and per heartbeat
//! interval, the heartbeats each carries, and the process CPU time per Raft
//! message handled.
//!
//! Heartbeats travel batched per peer (`network::Batcher`), so a peer gets
//! about one message per tick whatever the groups it shares; before they were
//! batched, it got one per shared group. The CPU per message shows whether routing one costs
//! more with more groups: a scan of every group per message would grow it with
//! them; a lookup keeps it flat.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use calimero_log::{
    BoxFuture, Config, Engine, Group, GroupId, Machine, NodeId, RpcRequest, RpcResponse, StateTx,
    Transport,
};

/// An idle machine: the bench sends no commands.
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
struct Counts {
    /// Every message node 1 sent node 2.
    to_follower: AtomicU64,
    /// The heartbeats (empty appends) they carried.
    heartbeats: AtomicU64,
    /// Every Raft message any node handled (a batch counts its items).
    handled: AtomicU64,
}

#[derive(Default)]
struct Net {
    engines: RwLock<HashMap<NodeId, Arc<Engine>>>,
    counts: Counts,
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
        let (net, me) = (Arc::clone(&self.0), self.1);
        Box::pin(async move {
            let c = &net.counts;
            if me == 1 && target == 2 {
                let _n = c.to_follower.fetch_add(1, Ordering::Relaxed);
                if matches!(&request, RpcRequest::Append(r) if r.entries.is_empty()) {
                    let _n = c.heartbeats.fetch_add(1, Ordering::Relaxed);
                }
            }
            let _n = c.handled.fetch_add(1, Ordering::Relaxed);
            let engine = net
                .engines
                .read()
                .map_err(|e| e.to_string())?
                .get(&target)
                .cloned();
            match engine {
                Some(e) => Ok(e.handle_rpc(group, request).await),
                None => Err("no such node".into()),
            }
        })
    }

    /// One message, however many heartbeats it carries.
    fn send_batch(
        &self,
        target: NodeId,
        _addr: String,
        items: Vec<(GroupId, RpcRequest)>,
    ) -> BoxFuture<Result<Vec<Result<RpcResponse, String>>, String>> {
        let (net, me) = (Arc::clone(&self.0), self.1);
        Box::pin(async move {
            let c = &net.counts;
            let n = items.len() as u64;
            if me == 1 && target == 2 {
                let _n = c.to_follower.fetch_add(1, Ordering::Relaxed);
                let _n = c.heartbeats.fetch_add(n, Ordering::Relaxed);
            }
            let _n = c.handled.fetch_add(n, Ordering::Relaxed);
            let engine = net
                .engines
                .read()
                .map_err(|e| e.to_string())?
                .get(&target)
                .cloned()
                .ok_or("no such node")?;
            Ok(engine
                .handle_rpc_batch(items)
                .await
                .into_iter()
                .map(Ok)
                .collect())
        })
    }
}

/// This process's user and system CPU time (Linux `/proc/self/stat`); zero
/// where it cannot be read.
fn cpu_time() -> Duration {
    let ticks = std::fs::read_to_string("/proc/self/stat")
        .ok()
        .and_then(|s| {
            // Fields after the command name (which ends at the last ')').
            let rest = s.rsplit_once(')')?.1;
            let f: Vec<&str> = rest.split_whitespace().collect();
            Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
        })
        .unwrap_or(0);
    // USER_HZ is 100 on every Linux the bench runs on.
    Duration::from_millis(ticks * 10)
}

struct Row {
    groups: u64,
    per_sec: f64,
    per_interval: f64,
    heartbeats_per_message: f64,
    cpu_per_message: Duration,
}

#[allow(clippy::cast_precision_loss, reason = "printed rates")]
async fn measure(groups: u64, window: Duration) -> Row {
    let config = Config::default();
    let net = Arc::new(Net::default());
    let mut dirs = Vec::new();
    let mut engines = Vec::new();
    let mut leaders: Vec<Arc<Group<Idle>>> = Vec::new();
    for id in 1..=3 {
        let dir = tempfile::tempdir().expect("a temp dir");
        let engine = Engine::open(
            id,
            dir.path(),
            &[7; 32],
            Arc::new(Wire(Arc::clone(&net), id)),
            config.clone(),
        )
        .expect("the engine opens");
        let _prev = net
            .engines
            .write()
            .expect("not poisoned")
            .insert(id, Arc::clone(&engine));
        for g in 1..=groups {
            let group = engine.start_group(g, Idle).await.expect("the group starts");
            if id == 1 {
                leaders.push(group);
            }
        }
        engines.push(engine);
        dirs.push(dir);
    }
    let voters: BTreeMap<NodeId, String> = (1..=3).map(|i| (i, format!("node-{i}"))).collect();
    for chunk in leaders.chunks(64) {
        let inits = chunk.iter().map(|g| {
            let (g, voters) = (Arc::clone(g), voters.clone());
            async move { g.initialize(voters).await }
        });
        for r in futures_join(inits).await {
            r.expect("the group initializes");
        }
    }
    let deadline = Instant::now() + Duration::from_secs(120);
    while !leaders.iter().all(|g| g.status().is_leader) {
        assert!(
            Instant::now() < deadline,
            "node 1 does not lead every group"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Let the initial appends settle, then count an idle window.
    tokio::time::sleep(Duration::from_secs(2)).await;
    let c = &net.counts;
    let (sent0, hb0, handled0) = (
        c.to_follower.load(Ordering::Relaxed),
        c.heartbeats.load(Ordering::Relaxed),
        c.handled.load(Ordering::Relaxed),
    );
    let cpu0 = cpu_time();
    tokio::time::sleep(window).await;
    let cpu = cpu_time().saturating_sub(cpu0);
    let sent = c.to_follower.load(Ordering::Relaxed) - sent0;
    let heartbeats = c.heartbeats.load(Ordering::Relaxed) - hb0;
    let handled = c.handled.load(Ordering::Relaxed) - handled0;
    for e in &engines {
        e.shutdown().await;
    }
    let per_sec = sent as f64 / window.as_secs_f64();
    Row {
        groups,
        per_sec,
        per_interval: per_sec * config.heartbeat_ms as f64 / 1000.0,
        heartbeats_per_message: heartbeats as f64 / sent.max(1) as f64,
        cpu_per_message: cpu / u32::try_from(handled.max(1)).unwrap_or(u32::MAX),
    }
}

/// Await every future in `futs` concurrently, in order.
async fn futures_join<F: std::future::Future + Send + 'static>(
    futs: impl Iterator<Item = F>,
) -> Vec<F::Output>
where
    F::Output: Send,
{
    let handles: Vec<_> = futs.map(tokio::spawn).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("the task completes"));
    }
    out
}

fn main() {
    let sizes: Vec<u64> = std::env::var("HEARTBEAT_GROUPS")
        .unwrap_or_else(|_| "1,100,1000".to_owned())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let window = Duration::from_secs(5);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime");
    println!(
        "groups | msgs node1->node2 /s | per heartbeat interval | heartbeats per msg | CPU per Raft message"
    );
    for groups in sizes {
        let r = rt.block_on(measure(groups, window));
        println!(
            "{:>6} | {:>20.1} | {:>22.1} | {:>18.1} | {:?}",
            r.groups, r.per_sec, r.per_interval, r.heartbeats_per_message, r.cpu_per_message
        );
    }
}
