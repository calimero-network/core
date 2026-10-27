//! What one write through `calimero-log` costs: a proposal appended, committed
//! and applied on one group, on a single node and on a 3-node group in one
//! process (the in-memory transport of `tests/cluster.rs`), one at a time and
//! 64 in flight. Beside it, one RocksDB write synced and unsynced on the same
//! disk, the unit a commit's latency is made of.

use std::collections::{BTreeMap, HashMap};
use std::hint::black_box;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use calimero_log::{
    BoxFuture, Config, Engine, Group, GroupId, Machine, NodeId, Proposal, RpcRequest, RpcResponse,
    StateTx, Transport,
};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use tokio::runtime::Runtime;

/// The bytes of one command: about a small app call's entry.
const COMMAND_BYTES: usize = 256;

/// Proposals in flight at once in the concurrent rows.
const IN_FLIGHT: usize = 64;

/// A machine on disk, like an app group: each command is one state row.
#[derive(Default)]
struct Rows;

impl Machine for Rows {
    fn apply(&mut self, index: u64, command: &[u8], state: &mut StateTx<'_>) -> Vec<u8> {
        state.put(index.to_be_bytes().to_vec(), command.to_vec());
        Vec::new()
    }

    fn restore(&mut self, _data: &[u8]) -> eyre::Result<()> {
        Ok(())
    }
}

#[derive(Default)]
struct Net {
    engines: RwLock<HashMap<NodeId, Arc<Engine>>>,
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
}

/// `n` engines hosting group 1, and the one that leads it.
struct Cluster {
    _dirs: Vec<tempfile::TempDir>,
    engines: Vec<Arc<Engine>>,
    leader: Arc<Group<Rows>>,
}

async fn cluster(n: u64) -> Cluster {
    let config = Config {
        heartbeat_ms: 50,
        election_min_ms: 300,
        election_max_ms: 600,
        // Off the measured path: a snapshot every few thousand entries.
        snapshot_every: 5_000,
    };
    let net = Arc::new(Net::default());
    let mut dirs = Vec::new();
    let mut engines = Vec::new();
    let mut groups = Vec::new();
    for id in 1..=n {
        let dir = tempfile::tempdir().expect("a temp dir");
        let engine = Engine::open(
            id,
            dir.path(),
            &[7; 32],
            Arc::new(Wire(Arc::clone(&net))),
            config.clone(),
        )
        .expect("the engine opens");
        let group = engine.start_group(1, Rows).await.expect("the group starts");
        let _prev = net
            .engines
            .write()
            .expect("not poisoned")
            .insert(id, Arc::clone(&engine));
        engines.push(engine);
        groups.push(group);
        dirs.push(dir);
    }
    let voters: BTreeMap<NodeId, String> = (1..=n).map(|i| (i, format!("node-{i}"))).collect();
    groups[0]
        .initialize(voters)
        .await
        .expect("the group initializes");
    for _ in 0..200 {
        if let Some(g) = groups.iter().find(|g| g.status().is_leader) {
            return Cluster {
                _dirs: dirs,
                engines,
                leader: Arc::clone(g),
            };
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no leader elected");
}

impl Cluster {
    async fn stop(self) {
        for e in &self.engines {
            e.shutdown().await;
        }
    }
}

async fn propose(group: &Group<Rows>) {
    let cmd = vec![0xAB_u8; COMMAND_BYTES];
    let _ = group
        .propose(|_m, _applied| Proposal::Append(cmd))
        .await
        .expect("the leader appends");
}

fn append_commit(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .enable_all()
        .build()
        .expect("a runtime");
    let mut group = c.benchmark_group("append_commit");
    group.sample_size(20);
    group.measurement_time(Duration::from_secs(5));
    for nodes in [1_u64, 3] {
        let cluster = rt.block_on(cluster(nodes));
        group.throughput(Throughput::Elements(1));
        group.bench_function(
            BenchmarkId::new("one_in_flight", format!("{nodes}_node")),
            |b| {
                b.iter_custom(|iters| timed(&rt, iters, 1, &cluster.leader));
            },
        );
        group.throughput(Throughput::Elements(IN_FLIGHT as u64));
        group.bench_function(
            BenchmarkId::new(format!("{IN_FLIGHT}_in_flight"), format!("{nodes}_node")),
            |b| b.iter_custom(|iters| timed(&rt, iters, IN_FLIGHT, &cluster.leader)),
        );
        rt.block_on(cluster.stop());
    }
    group.finish();
}

/// `iters` rounds of `width` proposals started together, each round waiting
/// for all of them.
fn timed(rt: &Runtime, iters: u64, width: usize, leader: &Arc<Group<Rows>>) -> Duration {
    rt.block_on(async {
        let start = Instant::now();
        for _ in 0..iters {
            let tasks: Vec<_> = (0..width)
                .map(|_| {
                    let leader = Arc::clone(leader);
                    tokio::spawn(async move { propose(&leader).await })
                })
                .collect();
            for t in tasks {
                t.await.expect("the proposal task ran");
            }
        }
        start.elapsed()
    })
}

/// One RocksDB write of a small batch, synced (what a log append and a vote
/// are) and unsynced (what an apply and `save_committed` are), on the disk the
/// bench's temp dir is on. Throughput can be held against
/// 1 ÷ (syncs per write × this).
fn rocksdb_write(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut options = rocksdb::Options::default();
    options.create_if_missing(true);
    let db = rocksdb::DB::open(&options, dir.path()).expect("rocksdb opens");
    let value = vec![0xCD_u8; COMMAND_BYTES];
    let mut group = c.benchmark_group("rocksdb_write");
    group.throughput(Throughput::Elements(1));
    for synced in [true, false] {
        let mut opts = rocksdb::WriteOptions::default();
        opts.set_sync(synced);
        let mut key = 0_u64;
        let name = if synced { "synced" } else { "unsynced" };
        group.bench_function(name, |b| {
            b.iter(|| {
                key += 1;
                let mut batch = rocksdb::WriteBatch::default();
                batch.put(key.to_be_bytes(), &value);
                db.write_opt(batch, &opts).expect("the write lands");
                black_box(key)
            });
        });
    }
    group.finish();
}

criterion_group!(benches, append_commit, rocksdb_write);
criterion_main!(benches);
