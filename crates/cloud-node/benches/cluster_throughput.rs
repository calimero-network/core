//! Call throughput through a 3-node cluster in one process: the real node,
//! sealed channels, mock attestation, the kv-store fixture's `set`, each call
//! sent to the app's leader. Calls per second and p50 / p99 latency for:
//!
//! - one app, 1 call in flight;
//! - one app, 64 in flight;
//! - two apps, 64 in flight each, both led by the same node;
//! - two apps, 64 in flight each, led by different nodes.
//!
//! Two apps at about twice one app's rate means the limit is per app (each
//! app's group proposes one call at a time, `calimero_log::Group::propose`);
//! two apps sharing one app's rate means it is per node. Beside it, one synced
//! RocksDB write on the same disk, so the rate can be held against
//! 1 ÷ (syncs per call × sync time).
//!
//! ```text
//! cargo bench -p calimero-cloud-node --bench cluster_throughput -- [--secs 10] [--out FILE.json]
//! ```
//!
//! Needs `kv_store.wasm` (see `tests/harness/cluster.rs`). Not a criterion
//! bench: one run is a few seconds of load per row, reported once.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_cloud_client::Client;
use serde_json::{json, Value};

#[path = "../tests/harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this bench does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, Cluster};

/// Calls each worker keeps in flight: one at a time, back to back.
const WIDE: usize = 64;

/// Unmeasured load before each row, so leases, sessions and caches are warm.
const WARM_UP: Duration = Duration::from_secs(2);

/// Synced writes timed for the sync figure.
const SYNC_SAMPLES: usize = 200;

struct Row {
    name: &'static str,
    in_flight: usize,
    /// The node leading each app when the row ended (1-based; 0: none).
    leaders: Vec<u64>,
    /// Each app's own calls per second.
    per_app: Vec<f64>,
    calls: usize,
    errors: usize,
    secs: f64,
    p50_ms: f64,
    p99_ms: f64,
}

impl Row {
    fn per_sec(&self) -> f64 {
        self.calls as f64 / self.secs
    }

    fn json(&self) -> Value {
        json!({
            "name": self.name,
            "apps": self.per_app.len(),
            "inFlightPerApp": self.in_flight,
            "leaderNodes": self.leaders,
            "callsPerSecondPerApp": self.per_app,
            "calls": self.calls,
            "errors": self.errors,
            "seconds": self.secs,
            "callsPerSecond": self.per_sec(),
            "p50Ms": self.p50_ms,
            "p99Ms": self.p99_ms,
        })
    }
}

fn pct(sorted: &[Duration], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)].as_secs_f64() * 1e3
}

/// One app under load, and the client (signed in on its leader's node) that
/// calls it.
struct Target {
    client: Arc<Client>,
    app: String,
}

/// `in_flight` workers per target, each calling `set` back to back, measured
/// for `secs` after the warm-up.
async fn load(
    c: &Cluster,
    name: &'static str,
    targets: &[Target],
    in_flight: usize,
    secs: Duration,
) -> Row {
    let measuring = Arc::new(AtomicBool::new(false));
    let stop = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for (t, target) in targets.iter().enumerate() {
        for w in 0..in_flight {
            let client = Arc::clone(&target.client);
            let app = target.app.clone();
            let measuring = Arc::clone(&measuring);
            let stop = Arc::clone(&stop);
            workers.push(tokio::spawn(async move {
                let mut times = Vec::new();
                let mut errors = 0;
                let mut n = 0_u64;
                while !stop.load(Ordering::Relaxed) {
                    n += 1;
                    let args = json!({ "key": format!("t{t}-w{w}-{n}"), "value": "x" });
                    let start = Instant::now();
                    let ok = client.call(&app, "set", args, None).await.is_ok();
                    let took = start.elapsed();
                    if !measuring.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                        continue;
                    }
                    if ok {
                        times.push(took);
                    } else {
                        errors += 1;
                    }
                }
                (t, times, errors)
            }));
        }
    }
    tokio::time::sleep(WARM_UP).await;
    measuring.store(true, Ordering::Relaxed);
    let start = Instant::now();
    tokio::time::sleep(secs).await;
    stop.store(true, Ordering::Relaxed);
    let measured = start.elapsed().as_secs_f64();
    let mut all = Vec::new();
    let mut per_app = vec![0_usize; targets.len()];
    let mut errors = 0;
    for w in workers {
        let (t, times, e) = w.await.expect("a worker ran");
        per_app[t] += times.len();
        all.extend(times);
        errors += e;
    }
    all.sort();
    Row {
        name,
        in_flight,
        leaders: targets
            .iter()
            .map(|t| c.app_leader(&t.app).map_or(0, |i| i as u64 + 1))
            .collect(),
        per_app: per_app.iter().map(|n| *n as f64 / measured).collect(),
        calls: all.len(),
        errors,
        secs: measured,
        p50_ms: pct(&all, 0.5),
        p99_ms: pct(&all, 0.99),
    }
}

/// Make node `to` (an index into the cluster) lead `app`: it calls an
/// election, which it wins once its log is as long as the leader's
/// (`Group::transfer_leader` is not supported on openraft 0.9).
async fn lead_on(c: &Cluster, app: &str, to: usize) {
    let target = c.nodes[to]
        .as_ref()
        .and_then(|r| r.node.hosted(app))
        .expect("every node hosts the app");
    for _ in 0..50 {
        if c.app_leader(app) == Some(to) {
            return;
        }
        let _ = target.raft().trigger().elect().await;
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    panic!("{app} never moved to node {}", to + 1);
}

/// The time of one synced RocksDB write of a small batch, on the disk the
/// cluster's temp dirs are on: (median, mean) in ms, and the unsynced median.
fn sync_time() -> (f64, f64, f64) {
    let dir = tempfile::tempdir().expect("a temp dir");
    let mut options = rocksdb::Options::default();
    options.create_if_missing(true);
    let db = rocksdb::DB::open(&options, dir.path()).expect("rocksdb opens");
    let time_writes = |synced: bool| {
        let mut opts = rocksdb::WriteOptions::default();
        opts.set_sync(synced);
        let mut times = Vec::with_capacity(SYNC_SAMPLES);
        for i in 0..SYNC_SAMPLES {
            let mut batch = rocksdb::WriteBatch::default();
            let key = [u64::from(synced), i as u64];
            batch.put(format!("{key:?}"), [0xAB_u8; 256]);
            let start = Instant::now();
            db.write_opt(batch, &opts).expect("the write lands");
            times.push(start.elapsed());
        }
        let mean = times.iter().map(Duration::as_secs_f64).sum::<f64>() / times.len() as f64;
        times.sort();
        (pct(&times, 0.5), mean * 1e3)
    };
    let (synced, mean) = time_writes(true);
    let (unsynced, _) = time_writes(false);
    (synced, mean, unsynced)
}

async fn signed_in(
    c: &Cluster,
    pk: &mut calimero_identity::testing::SoftAuthenticator,
    node: usize,
) -> Arc<Client> {
    let client = c.client(node);
    let _s = client.sign_in(pk).await.expect("signs in");
    Arc::new(client)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let arg = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let secs = Duration::from_secs_f64(
        arg("--secs")
            .map_or(Ok(10.0), |s| s.parse())
            .expect("--secs is a number"),
    );
    let out = arg("--out");

    let (sync_ms, sync_mean_ms, unsynced_ms) = sync_time();
    println!("one synced RocksDB write: median {sync_ms:.3} ms (mean {sync_mean_ms:.3} ms); unsynced {unsynced_ms:.3} ms");

    // merod's Raft timing, not the tests' faster one: under 128 calls in
    // flight the tests' 500 ms election timeout can fire on a busy machine.
    let mut c = Cluster::start_with(3, |cfg| cfg.raft = calimero_log::Config::default()).await;
    let (owner, mut pk) = signed_up(&c, 0, "bench").await;
    let bundle = owner.upload_bundle(kv_wasm()).await.expect("uploads");
    let a = owner
        .create_app("a", &bundle, json!({}))
        .await
        .expect("app a");
    let b = owner
        .create_app("b", &bundle, json!({}))
        .await
        .expect("app b");
    let _ = owner
        .call(&a, "set", json!({ "key": "warm", "value": "x" }), None)
        .await
        .expect("a answers");
    let _ = owner
        .call(&b, "set", json!({ "key": "warm", "value": "x" }), None)
        .await
        .expect("b answers");

    let mut clients = Vec::new();
    for node in 0..3 {
        clients.push(signed_in(&c, &mut pk, node).await);
    }
    let target = |app: &str, node: usize| Target {
        client: Arc::clone(&clients[node]),
        app: app.to_owned(),
    };

    let mut rows = Vec::new();
    // Both apps led by node 1 first, then b moved to node 2.
    lead_on(&c, &a, 0).await;
    lead_on(&c, &b, 0).await;
    for (name, apps, in_flight) in [
        ("1 app, 1 in flight", 1, 1),
        ("1 app, 64 in flight", 1, WIDE),
        ("2 apps, 64 in flight each, same leader node", 2, WIDE),
    ] {
        let targets: Vec<Target> = [&a, &b][..apps].iter().map(|app| target(app, 0)).collect();
        rows.push(load(&c, name, &targets, in_flight, secs).await);
    }
    lead_on(&c, &b, 1).await;
    let targets = [target(&a, 0), target(&b, 1)];
    rows.push(
        load(
            &c,
            "2 apps, 64 in flight each, different leader nodes",
            &targets,
            WIDE,
            secs,
        )
        .await,
    );
    for i in 0..3 {
        c.stop(i).await;
    }

    println!();
    println!("| scenario | leader node(s) | calls/s | per app | p50 | p99 | errors |");
    println!("|---|---|---|---|---|---|---|");
    for r in &rows {
        let per_app: Vec<String> = r.per_app.iter().map(|x| format!("{x:.1}")).collect();
        println!(
            "| {} | {:?} | {:.1} | {} | {:.2} ms | {:.2} ms | {} |",
            r.name,
            r.leaders,
            r.per_sec(),
            per_app.join(" + "),
            r.p50_ms,
            r.p99_ms,
            r.errors
        );
    }
    let one = rows[1].per_sec();
    let same = rows[2].per_sec();
    let apart = rows[3].per_sec();
    println!(
        "\n2 apps / 1 app at 64 in flight: {:.2}x on one leader node, {:.2}x on two; 1 ÷ sync time = {:.0}/s",
        same / one,
        apart / one,
        1e3 / sync_ms
    );
    if let Some(out) = out {
        let report = json!({
            "bench": "cluster_throughput",
            "nodes": 3,
            "secondsPerRow": secs.as_secs_f64(),
            "cores": std::thread::available_parallelism().map_or(0, |n| n.get()),
            "syncedWriteMedianMs": sync_ms,
            "syncedWriteMeanMs": sync_mean_ms,
            "unsyncedWriteMedianMs": unsynced_ms,
            "rows": rows.iter().map(Row::json).collect::<Vec<_>>(),
            "twoAppsOverOneSameLeader": same / one,
            "twoAppsOverOneDifferentLeaders": apart / one,
        });
        std::fs::write(
            &out,
            serde_json::to_vec_pretty(&report).expect("serializes"),
        )
        .expect("writes the report");
        println!("wrote {out}");
    }
}
