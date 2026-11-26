//! Live views under load: a 3-node cluster in one process, the kv fixture's subscribable
//! views (`apps/fixture-kv`: `get`, `entries`), watched over `GET /events?watch=true`
//! from one node while another node's client writes. Rows, printed once:
//!
//! - **view run**: one `get` / `entries` run in the engine with and without the
//!   read-set recorder (µs), the cost a watch adds to each re-run;
//! - **capacity**: 64 → 2,048 watches on one node (streams of 64, then one watch per
//!   stream), a steady write rate touching one watch per write: change → frame p50 / p99
//!   and the process's resident set, and the watch the node refuses past its cap;
//! - **updates/s**: every watch dirty all the time (`entries` reads the whole table):
//!   frames per second the node delivers against the one per watch per interval asked;
//! - **time per update**: write → applied on the watcher's node → frame, with the view
//!   run from the first row; the rest is the read-set check, the access check, the
//!   serialization and the SSE hop;
//! - **same view, many watchers**: 1 → 1,000 watches of one `get`: time to the first and
//!   the last frame (each watch runs the view itself: re-runs per change = watchers);
//! - **burst**: writes to one key as fast as the cluster takes them, 64 watchers:
//!   frames per watch per second and bytes;
//! - **system reads**: `myRole` watchers (a view that reads the directory) while 64
//!   sessions refresh: view runs per second and CPU for answers that never change;
//! - **polling baseline**: the same `get` polled at 500 ms / 1 s / 4 s by 256 clients.
//!
//! ```text
//! cargo bench -p calimero-node --bench live_views -- [--secs 10] [--out FILE.json] [--label NAME] [--rows capacity,fanout,..]
//! ```
//!
//! The watch interval is the node's default (1 s); a faster one depends on the
//! interval the app sets for its view. View runs are counted from the node's
//! `calimero_wasm_run_duration_seconds{kind="view"}` and CPU is the whole process's
//! (`getrusage`): three nodes, the clients and the bench together. All three nodes, the clients and the watchers
//! share this machine's cores. Not a criterion bench: each row is a few seconds of
//! load, reported once.

#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "a bench's panic is its report, and it prints its measurements"
)]

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    Budget, Caller, Directory, Engine, HostCore, Invocation, InvocationKind, OrgRole, Prepared,
    Status,
};
use calimero_client::Client;
use futures_util::StreamExt;
use serde_json::{json, Value};
use tokio::sync::mpsc;

#[path = "../tests/harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/it/cluster.rs, which uses the parts this bench does not"
)]
mod harness;

use harness::{kv_wasm, signed_up, typed, Cluster};

/// Watches one stream holds at most (`watch::MAX_WATCHES_PER_STREAM`).
const PER_STREAM: usize = calimero_node::watch::MAX_WATCHES_PER_STREAM;

fn pct(sorted: &[Duration], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i.min(sorted.len() - 1)].as_secs_f64() * 1e3
}

fn p50_p99(mut v: Vec<Duration>) -> (f64, f64) {
    v.sort();
    (pct(&v, 0.5), pct(&v, 0.99))
}

/// This process's resident set in MiB (`ps`, macOS and Linux).
fn rss_mib() -> f64 {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(0.0, |kib| kib / 1024.0)
}

/// View runs every node in this process has made, and their summed wall time (s), from
/// `calimero_wasm_run_duration_seconds{kind="view"}` (the metrics registry is process-wide).
fn view_runs() -> (u64, f64) {
    let mut out = String::new();
    calimero_node::metrics::render(&mut out);
    let field = |name: &str| {
        out.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    (
        field("calimero_wasm_run_duration_seconds_count{kind=\"view\"}") as u64,
        field("calimero_wasm_run_duration_seconds_sum{kind=\"view\"}"),
    )
}

/// `calimero_live_view_runs_total{outcome}` (shared, deferred), when the node reports it.
fn live_outcomes() -> (u64, u64) {
    let mut out = String::new();
    calimero_node::metrics::render(&mut out);
    let field = |o: &str| {
        let name = format!("calimero_live_view_runs_total{{outcome=\"{o}\"}}");
        out.lines()
            .find_map(|l| l.strip_prefix(name.as_str()))
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.0) as u64
    };
    (field("shared"), field("deferred"))
}

/// CPU time (user + system) this process has used, in seconds.
fn cpu_secs() -> f64 {
    // SAFETY: getrusage writes only the struct it is handed.
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: as above.
    let _ = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    let t = |v: libc::timeval| v.tv_sec as f64 + v.tv_usec as f64 / 1e6;
    t(u.ru_utime) + t(u.ru_stime)
}

/// What a measured window cost: view runs, their wall time, and process CPU.
#[derive(Clone, Copy, Default)]
struct Cost {
    runs: u64,
    /// Watches answered by another watch's run of the same view.
    shared: u64,
    /// Watch runs the node's budget put off.
    deferred: u64,
    run_secs: f64,
    cpu: f64,
    wall: f64,
}

impl Cost {
    fn start() -> (u64, f64, f64, Instant, (u64, u64)) {
        let (n, s) = view_runs();
        (n, s, cpu_secs(), Instant::now(), live_outcomes())
    }

    fn since((n, s, cpu, at, (sh, de)): (u64, f64, f64, Instant, (u64, u64))) -> Self {
        let (n2, s2) = view_runs();
        let (sh2, de2) = live_outcomes();
        Self {
            runs: n2.saturating_sub(n),
            shared: sh2.saturating_sub(sh),
            deferred: de2.saturating_sub(de),
            run_secs: s2 - s,
            cpu: cpu_secs() - cpu,
            wall: at.elapsed().as_secs_f64(),
        }
    }

    /// CPU cores busy on average (process CPU s per wall s).
    fn cores(&self) -> f64 {
        self.cpu / self.wall.max(1e-9)
    }

    fn json(&self) -> Value {
        json!({
            "viewRuns": self.runs, "viewRunSeconds": self.run_secs,
            "sharedAnswers": self.shared, "deferredRuns": self.deferred,
            "processCpuSeconds": self.cpu, "wallSeconds": self.wall, "coresBusy": self.cores(),
        })
    }
}

// --- view run in the engine ---------------------------------------------------------

struct NoDirectory;

impl Directory for NoDirectory {
    fn org(&self) -> Option<String> {
        None
    }
    fn app_owner(&self) -> String {
        "owner".to_owned()
    }
    fn org_role(&self, _account: &str) -> Option<OrgRole> {
        None
    }
    fn in_team(&self, _account: &str, _team: &str) -> bool {
        false
    }
    fn team_exists(&self, _team: &str) -> bool {
        false
    }
    fn orgs_of(&self, _account: &str) -> Vec<(String, OrgRole)> {
        Vec::new()
    }
}

fn host(
    state: &BTreeMap<Vec<u8>, Vec<u8>>,
    kind: InvocationKind,
    method: &str,
    args: &Value,
) -> HostCore {
    let inv = Invocation {
        kind,
        method: method.to_owned(),
        args: serde_json::to_vec(args).unwrap(),
        caller: Caller::Account("alice".to_owned()),
        entry_ts_ms: 1_700_000_000_000,
        id_term: 1,
        id_seq: 1,
        seed: [7; 32],
        timer: None,
        compute: None,
    };
    HostCore::new(
        "app",
        Arc::new(state.clone()),
        1,
        inv,
        Arc::new(NoDirectory),
    )
    .unwrap()
}

/// Runs `method` and applies its diff to `state`.
fn apply(
    e: &WasmtimeEngine,
    app: &Prepared,
    state: &mut BTreeMap<Vec<u8>, Vec<u8>>,
    kind: InvocationKind,
    method: &str,
    args: Value,
) {
    let out = e.invoke(app, host(state, kind, method, &args), Budget::call());
    assert!(
        matches!(out.status, Status::Ok),
        "{method}: {:?}",
        out.status
    );
    for w in out.diff {
        match w.value {
            Some(v) => {
                let _ = state.insert(w.key, v);
            }
            None => {
                let _ = state.remove(&w.key);
            }
        }
    }
}

struct ViewCost {
    name: String,
    plain_us: f64,
    recorded_us: f64,
    read_set_bytes: usize,
}

/// One view's median run time without and with the recorder, over `runs` runs.
fn view_cost(
    e: &WasmtimeEngine,
    app: &Prepared,
    state: &BTreeMap<Vec<u8>, Vec<u8>>,
    name: String,
    method: &str,
    args: &Value,
    runs: usize,
) -> ViewCost {
    let time = |record: bool| {
        let mut times = Vec::with_capacity(runs);
        let mut bytes = 0;
        for _ in 0..runs {
            let mut h = host(state, InvocationKind::View, method, args);
            if record {
                h = h.with_read_set();
            }
            let start = Instant::now();
            let out = e.invoke(app, h, Budget::call());
            times.push(start.elapsed());
            assert!(
                matches!(out.status, Status::Ok),
                "{method}: {:?}",
                out.status
            );
            bytes = out.read_set.map_or(0, |s| s.bytes());
        }
        times.sort();
        (times[runs / 2].as_secs_f64() * 1e6, bytes)
    };
    let (plain_us, _) = time(false);
    let (recorded_us, read_set_bytes) = time(true);
    ViewCost {
        name,
        plain_us,
        recorded_us,
        read_set_bytes,
    }
}

fn view_costs() -> Vec<ViewCost> {
    let cache = tempfile::tempdir().unwrap();
    let e = WasmtimeEngine::new(cache.path().to_path_buf(), &[5; 32]).unwrap();
    let app = e.prepare("kv", &kv_wasm()).unwrap();
    let mut rows = Vec::new();
    for entries in [10_usize, 100, 1_000] {
        let mut state = BTreeMap::new();
        apply(
            &e,
            &app,
            &mut state,
            InvocationKind::Init,
            "init",
            json!({}),
        );
        for i in 0..entries {
            let args = json!({ "key": format!("k{i:04}"), "value": format!("value-{i}") });
            apply(&e, &app, &mut state, InvocationKind::Call, "set", args);
        }
        let get = json!({ "key": "k0001" });
        rows.push(view_cost(
            &e,
            &app,
            &state,
            format!("get, {entries} entries"),
            "get",
            &get,
            300,
        ));
        rows.push(view_cost(
            &e,
            &app,
            &state,
            format!("entries, {entries} entries"),
            "entries",
            &json!({}),
            100,
        ));
    }
    rows
}

// --- the cluster ---------------------------------------------------------------------

/// A frame a watcher got: the watch, its answer, when, and the bytes of its data.
struct Got {
    sub: String,
    answer: Value,
    at: Instant,
    bytes: usize,
}

/// The watches a row took, removed when it ends: a dropped stream's watches would
/// otherwise count against the node's cap for the resume grace (30 s).
#[derive(Default)]
struct Held {
    subs: Vec<String>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Held {
    async fn release(self, client: &Client, app: &str) {
        for t in self.tasks {
            t.abort();
        }
        for sub in self.subs {
            let _ = client.remove_watch(app, &sub).await;
        }
    }
}

/// Opens a stream on `client` and adds `views` to it; frames go to `tx`.
async fn stream_with(
    client: &Client,
    app: &str,
    views: &[(&str, Value)],
    tx: mpsc::UnboundedSender<Got>,
    held: &mut Held,
) -> Result<Vec<String>, String> {
    let stream = client
        .watch_stream(app, None)
        .await
        .map_err(|e| e.to_string())?;
    let mut subs = Vec::new();
    for (view, args) in views {
        let added = client
            .add_watch(app, &stream.id, view, args.clone(), None)
            .await
            .map_err(|e| e.to_string())?;
        subs.push(added.sub.clone());
        held.subs.push(added.sub);
    }
    let mut frames = stream.frames;
    let task = tokio::spawn(async move {
        while let Some((_, kind, data)) = frames.next().await {
            if kind != "view-result" {
                continue;
            }
            let bytes = data.to_string().len();
            let got = Got {
                sub: data["sub"].as_str().unwrap_or_default().to_owned(),
                answer: data["answer"].clone(),
                at: Instant::now(),
                bytes,
            };
            if tx.send(got).is_err() {
                break;
            }
        }
    });
    held.tasks.push(task);
    Ok(subs)
}

/// Frames gathered so far, and the receiver kept for more.
fn drain(rx: &mut mpsc::UnboundedReceiver<Got>) -> Vec<Got> {
    let mut out = Vec::new();
    while let Ok(g) = rx.try_recv() {
        out.push(g);
    }
    out
}

/// The app's last applied entry on node `i`.
fn applied(c: &Cluster, i: usize, app: &str) -> u64 {
    c.nodes[i]
        .as_ref()
        .and_then(|r| r.node.hosted(app))
        .map_or(0, |g| g.read_local(|m, _| m.state.last_index))
}

async fn set(client: &Client, app: &str, key: &str, value: &str) -> u64 {
    client
        .apps()
        .call(
            app,
            "set",
            Some(json!({ "key": key, "value": value })),
            None,
            None,
        )
        .await
        .map(|r| r.index)
        .expect("set lands")
}

struct Capacity {
    watches: usize,
    streams: usize,
    writes_per_sec: f64,
    p50_ms: f64,
    p99_ms: f64,
    missed: usize,
    rss_mib: f64,
    cost: Cost,
}

/// `watches` watches of `get(c{i})` on node `w`, in streams of `per_stream`; `secs` of
/// writes, one key per write, `rate` writes/s; change (write sent) → frame latency.
#[allow(clippy::too_many_arguments, reason = "a bench row's knobs")]
async fn capacity(
    c: &Cluster,
    watcher: &Arc<Client>,
    writer: &Arc<Client>,
    app: &str,
    watches: usize,
    per_stream: usize,
    rate: f64,
    secs: Duration,
) -> Capacity {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut held = Held::default();
    let mut sub_key = HashMap::new();
    let mut streams = 0;
    let mut i = 0;
    while i < watches {
        let n = per_stream.min(watches - i);
        let views: Vec<(&str, Value)> = (i..i + n)
            .map(|k| ("get", json!({ "key": format!("c{k}") })))
            .collect();
        let subs = stream_with(watcher, app, &views, tx.clone(), &mut held)
            .await
            .expect("the watches are taken");
        for (s, k) in subs.into_iter().zip(i..i + n) {
            let _ = sub_key.insert(s, k);
        }
        streams += 1;
        i += n;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = drain(&mut rx);
    let rss = rss_mib();
    // Writes spaced so one key is written at most once a second (the throttle interval).
    let gap = Duration::from_secs_f64(1.0 / rate);
    let sent: Arc<Mutex<HashMap<String, Instant>>> = Arc::default();
    let cost0 = Cost::start();
    let start = Instant::now();
    let mut n = 0_usize;
    let mut writers = Vec::new();
    while start.elapsed() < secs {
        let key = n % watches;
        // Unique per row too: an answer equal to the last one sent is not sent again.
        let value = format!("w{watches}-{n}");
        let _ = sent.lock().unwrap().insert(value.clone(), Instant::now());
        let client = Arc::clone(writer);
        let app = app.to_owned();
        writers.push(tokio::spawn(async move {
            let _ = set(&client, &app, &format!("c{key}"), &value).await;
        }));
        n += 1;
        tokio::time::sleep(gap).await;
    }
    for w in writers {
        let _ = w.await;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    let cost = Cost::since(cost0);
    let _ = c;
    let sent = sent.lock().unwrap().clone();
    let mut lat = Vec::new();
    for g in drain(&mut rx) {
        if let Some(at) = g.answer.as_str().and_then(|v| sent.get(v)) {
            lat.push(g.at.saturating_duration_since(*at));
        }
    }
    let missed = n.saturating_sub(lat.len());
    let (p50_ms, p99_ms) = p50_p99(lat);
    held.release(watcher, app).await;
    Capacity {
        watches,
        streams,
        writes_per_sec: n as f64 / secs.as_secs_f64(),
        p50_ms,
        p99_ms,
        missed,
        rss_mib: rss,
        cost,
    }
}

struct Updates {
    watches: usize,
    asked_per_sec: f64,
    frames_per_sec: f64,
    rss_mib: f64,
    cost: Cost,
}

/// `watches` watches of `entries` (the whole table: every write dirties all of them),
/// written to all the time; frames delivered per second over `secs`.
async fn updates(
    watcher: &Arc<Client>,
    writer: &Arc<Client>,
    app: &str,
    watches: usize,
    secs: Duration,
) -> Updates {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut held = Held::default();
    let mut i = 0;
    while i < watches {
        let n = PER_STREAM.min(watches - i);
        let views: Vec<(&str, Value)> = (0..n).map(|_| ("entries", json!({}))).collect();
        let _ = stream_with(watcher, app, &views, tx.clone(), &mut held)
            .await
            .expect("the watches are taken");
        i += n;
    }
    let stop = Arc::new(AtomicBool::new(false));
    let w = {
        let (client, app, stop) = (Arc::clone(writer), app.to_owned(), Arc::clone(&stop));
        tokio::spawn(async move {
            let mut n = 0_u64;
            while !stop.load(Ordering::Relaxed) {
                let _ = set(&client, &app, "u", &format!("{n}")).await;
                n += 1;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    tokio::time::sleep(Duration::from_secs(2)).await;
    let _ = drain(&mut rx);
    let cost0 = Cost::start();
    let start = Instant::now();
    tokio::time::sleep(secs).await;
    let frames = drain(&mut rx).len();
    let took = start.elapsed().as_secs_f64();
    let cost = Cost::since(cost0);
    let rss = rss_mib();
    stop.store(true, Ordering::Relaxed);
    let _ = w.await;
    held.release(watcher, app).await;
    Updates {
        watches,
        asked_per_sec: watches as f64,
        frames_per_sec: frames as f64 / took,
        rss_mib: rss,
        cost,
    }
}

struct Stages {
    samples: usize,
    commit: (f64, f64),
    applied: (f64, f64),
    frame: (f64, f64),
}

/// One watch per key on node `w`; writes on the leader, each key written every 1.2 s
/// (past the throttle): write → call answered (committed), → applied on `w`, → frame.
async fn stages(
    c: &Cluster,
    w: usize,
    watcher: &Arc<Client>,
    writer: &Arc<Client>,
    app: &str,
    samples: usize,
) -> Stages {
    const KEYS: usize = 12;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut held = Held::default();
    let views: Vec<(&str, Value)> = (0..KEYS)
        .map(|k| ("get", json!({ "key": format!("s{k}") })))
        .collect();
    let _ = stream_with(watcher, app, &views, tx, &mut held)
        .await
        .expect("watches");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = drain(&mut rx);
    let (mut commit, mut appl, mut frame) = (Vec::new(), Vec::new(), Vec::new());
    for n in 0..samples {
        let value = format!("s{n}");
        let start = Instant::now();
        let index = set(writer, app, &format!("s{}", n % KEYS), &value).await;
        let answered = start.elapsed();
        let mut on_w = None;
        let mut got = None;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && (on_w.is_none() || got.is_none()) {
            if on_w.is_none() && applied(c, w, app) >= index {
                on_w = Some(start.elapsed());
            }
            if got.is_none() {
                for g in drain(&mut rx) {
                    if g.answer.as_str() == Some(value.as_str()) {
                        got = Some(g.at.saturating_duration_since(start));
                    }
                }
            }
            tokio::time::sleep(Duration::from_micros(200)).await;
        }
        if let (Some(a), Some(f)) = (on_w, got) {
            commit.push(answered);
            appl.push(a);
            frame.push(f);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    held.release(watcher, app).await;
    Stages {
        samples: commit.len(),
        commit: p50_p99(commit),
        applied: p50_p99(appl),
        frame: p50_p99(frame),
    }
}

struct Fanout {
    watchers: usize,
    first: (f64, f64),
    last: (f64, f64),
    runs_per_change: f64,
    shared_per_change: f64,
    cpu_per_change_ms: f64,
}

/// `watchers` watches of one `get(hot)`; `rounds` writes 1.5 s apart: write → first and
/// last watcher's frame.
async fn fanout(
    watcher: &Arc<Client>,
    writer: &Arc<Client>,
    app: &str,
    watchers: usize,
    rounds: usize,
) -> Fanout {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut held = Held::default();
    let key = format!("hot{watchers}");
    let mut i = 0;
    while i < watchers {
        let n = PER_STREAM.min(watchers - i);
        let views: Vec<(&str, Value)> = (0..n).map(|_| ("get", json!({ "key": &key }))).collect();
        let _ = stream_with(watcher, app, &views, tx.clone(), &mut held)
            .await
            .expect("watches");
        i += n;
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = drain(&mut rx);
    let (mut first, mut last) = (Vec::new(), Vec::new());
    let mut cost = Cost::default();
    for r in 0..rounds {
        let cost0 = Cost::start();
        let value = format!("h{r}");
        let start = Instant::now();
        let _ = set(writer, app, &key, &value).await;
        let mut seen = 0;
        let mut t_first = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        while seen < watchers && Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
                Ok(Some(g)) if g.answer.as_str() == Some(value.as_str()) => {
                    seen += 1;
                    let t = g.at.saturating_duration_since(start);
                    t_first.get_or_insert(t);
                    if seen == watchers {
                        first.push(t_first.unwrap());
                        last.push(t);
                    }
                }
                _ => {}
            }
        }
        // Past the throttle interval: every watch is due at once next round.
        tokio::time::sleep(Duration::from_millis(1_200)).await;
        let c = Cost::since(cost0);
        cost.runs += c.runs;
        cost.shared += c.shared;
        cost.cpu += c.cpu;
        let _ = drain(&mut rx);
    }
    held.release(watcher, app).await;
    Fanout {
        watchers,
        first: p50_p99(first),
        last: p50_p99(last),
        runs_per_change: cost.runs as f64 / rounds as f64,
        shared_per_change: cost.shared as f64 / rounds as f64,
        cpu_per_change_ms: cost.cpu * 1e3 / rounds as f64,
    }
}

struct Burst {
    writes_per_sec: f64,
    frames_per_watch_per_sec: f64,
    bytes_per_sec: f64,
    cost: Cost,
}

/// 64 watchers of `get(burst)`; 32 writers write it back to back for `secs`.
async fn burst(watcher: &Arc<Client>, writer: &Arc<Client>, app: &str, secs: Duration) -> Burst {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut held = Held::default();
    let views: Vec<(&str, Value)> = (0..PER_STREAM)
        .map(|_| ("get", json!({ "key": "burst" })))
        .collect();
    let _ = stream_with(watcher, app, &views, tx, &mut held)
        .await
        .expect("watches");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = drain(&mut rx);
    let stop = Arc::new(AtomicBool::new(false));
    let writes = Arc::new(AtomicU64::new(0));
    let mut ws = Vec::new();
    for w in 0..32 {
        let (client, app, stop, writes) = (
            Arc::clone(writer),
            app.to_owned(),
            Arc::clone(&stop),
            Arc::clone(&writes),
        );
        ws.push(tokio::spawn(async move {
            let mut n = 0;
            while !stop.load(Ordering::Relaxed) {
                let _ = set(&client, &app, "burst", &format!("{w}-{n}")).await;
                let _ = writes.fetch_add(1, Ordering::Relaxed);
                n += 1;
            }
        }));
    }
    let cost0 = Cost::start();
    let start = Instant::now();
    tokio::time::sleep(secs).await;
    stop.store(true, Ordering::Relaxed);
    let took = start.elapsed().as_secs_f64();
    let cost = Cost::since(cost0);
    for w in ws {
        let _ = w.await;
    }
    let got = drain(&mut rx);
    let bytes: usize = got.iter().map(|g| g.bytes).sum();
    let mut per: HashMap<String, usize> = HashMap::new();
    for g in &got {
        *per.entry(g.sub.clone()).or_default() += 1;
    }
    let frames = per.values().sum::<usize>() as f64 / PER_STREAM as f64;
    held.release(watcher, app).await;
    Burst {
        writes_per_sec: writes.load(Ordering::Relaxed) as f64 / took,
        frames_per_watch_per_sec: frames / took,
        bytes_per_sec: bytes as f64 / took,
        cost,
    }
}

struct SystemReads {
    watches: usize,
    refreshes_per_sec: f64,
    frames_per_sec: f64,
    cost: Cost,
}

/// `watches` watches of `myRole` (a view that reads the directory) on `watcher`'s node
/// while 64 sessions refresh back to back (each refresh is a system entry here: the
/// harness exchanges every token in the log).
async fn system_reads(
    c: &Cluster,
    watcher: &Arc<Client>,
    pk: &mut calimero_identity::testing::SoftAuthenticator,
    app: &str,
    watches: usize,
    secs: Duration,
) -> SystemReads {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut held = Held::default();
    let mut i = 0;
    while i < watches {
        let n = PER_STREAM.min(watches - i);
        let views: Vec<(&str, Value)> = (0..n).map(|_| ("myRole", json!({}))).collect();
        let _ = stream_with(watcher, app, &views, tx.clone(), &mut held)
            .await
            .expect("watches");
        i += n;
    }
    let mut refreshers = Vec::new();
    for s in 0..64 {
        let client = c.client(s % 3);
        let _s = client.sign_in(pk).await.expect("signs in");
        refreshers.push(client);
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let _ = drain(&mut rx);
    let stop = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicU64::new(0));
    let mut rs = Vec::new();
    for client in refreshers {
        let (stop, done) = (Arc::clone(&stop), Arc::clone(&done));
        rs.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                if client.refresh().await.is_ok() {
                    let _ = done.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    let cost0 = Cost::start();
    tokio::time::sleep(secs).await;
    stop.store(true, Ordering::Relaxed);
    let cost = Cost::since(cost0);
    for r in rs {
        let _ = r.await;
    }
    let frames = drain(&mut rx).len();
    held.release(watcher, app).await;
    SystemReads {
        watches,
        refreshes_per_sec: done.load(Ordering::Relaxed) as f64 / cost.wall,
        frames_per_sec: frames as f64 / cost.wall,
        cost,
    }
}

struct Poll {
    interval_ms: u64,
    clients: usize,
    requests_per_sec: f64,
    bytes_per_sec: f64,
    p50_ms: f64,
    p99_ms: f64,
    cost: Cost,
}

/// `clients` pollers of `get(poll)` on `watcher`'s node, each every `interval`.
async fn poll(
    watcher: &Arc<Client>,
    app: &str,
    clients: usize,
    interval: Duration,
    secs: Duration,
) -> Poll {
    let stop = Arc::new(AtomicBool::new(false));
    let mut ps = Vec::new();
    for p in 0..clients {
        let (client, app, stop) = (Arc::clone(watcher), app.to_owned(), Arc::clone(&stop));
        ps.push(tokio::spawn(async move {
            // Spread the pollers over the interval.
            tokio::time::sleep(interval.mul_f64(p as f64 / clients as f64)).await;
            let mut times = Vec::new();
            let mut bytes = 0;
            let mut tick = tokio::time::interval(interval);
            while !stop.load(Ordering::Relaxed) {
                let _ = tick.tick().await;
                let start = Instant::now();
                if let Ok(r) = client
                    .apps()
                    .view(&app, "get", Some(json!({ "key": "poll" })), None, None)
                    .await
                {
                    times.push(start.elapsed());
                    bytes += serde_json::to_string(&r.result).map_or(0, |s| s.len());
                }
            }
            (times, bytes)
        }));
    }
    let cost0 = Cost::start();
    let start = Instant::now();
    tokio::time::sleep(secs).await;
    stop.store(true, Ordering::Relaxed);
    let took = start.elapsed().as_secs_f64();
    let cost = Cost::since(cost0);
    let (mut all, mut bytes) = (Vec::new(), 0);
    for p in ps {
        let (t, b) = p.await.unwrap();
        all.extend(t);
        bytes += b;
    }
    let n = all.len();
    let (p50_ms, p99_ms) = p50_p99(all);
    Poll {
        interval_ms: interval.as_millis() as u64,
        clients,
        requests_per_sec: n as f64 / took,
        bytes_per_sec: bytes as f64 / took,
        p50_ms,
        p99_ms,
        cost,
    }
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
    // A name for this run in the report, e.g. the commit or "before CPU fixes".
    let label = arg("--label").unwrap_or_default();
    // Comma-separated rows to run (view, stages, capacity, updates, fanout, burst,
    // system, poll); all when absent.
    let only = arg("--rows");
    let want = |row: &str| only.as_ref().is_none_or(|o| o.split(',').any(|r| r == row));
    let short = secs.mul_f64(0.5);

    println!("## view run in the engine (median µs)");
    let costs = if want("view") || want("stages") {
        view_costs()
    } else {
        Vec::new()
    };
    println!("| view | plain | recorded | overhead | read set |");
    println!("|---|---|---|---|---|");
    for v in &costs {
        println!(
            "| {} | {:.1} µs | {:.1} µs | {:+.1}% | {} B |",
            v.name,
            v.plain_us,
            v.recorded_us,
            (v.recorded_us / v.plain_us - 1.0) * 100.0,
            v.read_set_bytes
        );
    }

    let c = Cluster::start_with(3, |cfg| {
        cfg.raft = calimero_log::Config::default();
    })
    .await;
    let (owner, mut pk) = signed_up(&c, 0, "bench").await;
    let bundle = owner
        .packages()
        .bundle_put(kv_wasm())
        .await
        .map(|b| b.hash)
        .expect("uploads");
    let app: String = owner
        .apps()
        .create(typed(json!({ "name": "live", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .expect("the app");
    let _ = set(&owner, &app, "warm", "x").await;
    let leader = c.app_leader(&app).expect("a leader");
    let w = (leader + 1) % 3;
    let writer = signed_in(&c, &mut pk, leader).await;
    let watcher = signed_in(&c, &mut pk, w).await;
    println!(
        "\napp leader: node {}; watchers on node {}; cores: {}",
        leader + 1,
        w + 1,
        std::thread::available_parallelism().map_or(0, |n| n.get())
    );

    let st = if want("stages") {
        Some(stages(&c, w, &watcher, &writer, &app, 40).await)
    } else {
        None
    };
    if let Some(st) = &st {
        println!("\n## time per update (write on the leader, watch on another node)");
        let get_us = costs[0].recorded_us / 1e3;
        println!("| stage | p50 | p99 |");
        println!("|---|---|---|");
        println!(
            "| write committed (call answered) | {:.2} ms | {:.2} ms |",
            st.commit.0, st.commit.1
        );
        println!(
            "| applied on the watcher's node | {:.2} ms | {:.2} ms |",
            st.applied.0, st.applied.1
        );
        println!(
            "| frame at the client | {:.2} ms | {:.2} ms |",
            st.frame.0, st.frame.1
        );
        println!(
        "| of which view re-run (engine, recorded) | {get_us:.3} ms | |\n| of which check + access + send (frame − applied − view) | {:.2} ms | |",
        (st.frame.0 - st.applied.0 - get_us).max(0.0)
    );
        println!("({} samples)", st.samples);
    }

    println!("\n## capacity: watches on one node, writes touching one watch each");
    let mut caps = Vec::new();
    let mut cap_hit = String::new();
    if want("capacity") {
        for (watches, per) in [
            (64, PER_STREAM),
            (256, PER_STREAM),
            (1_024, PER_STREAM),
            (2_048, PER_STREAM),
        ] {
            caps.push(capacity(&c, &watcher, &writer, &app, watches, per, 50.0, short).await);
        }
        // The node's cap: 32 streams of 64 fill `MAX_WATCHES_PER_NODE`; one more is refused.
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut held = Held::default();
        let full: Vec<(&str, Value)> = vec![("get", json!({ "key": "over" })); PER_STREAM];
        for _ in 0..calimero_node::watch::MAX_WATCHES_PER_NODE / PER_STREAM {
            let _ = stream_with(&watcher, &app, &full, tx.clone(), &mut held).await;
        }
        let over = stream_with(&watcher, &app, &full[..1], tx, &mut held).await;
        held.release(&watcher, &app).await;
        println!("| watches | streams | writes/s | change→frame p50 | p99 | missed | view runs/write | CPU cores busy | RSS (3 nodes + clients) |");
        println!("|---|---|---|---|---|---|---|---|---|");
        for r in &caps {
            let writes = (r.writes_per_sec * r.cost.wall).max(1.0);
            println!(
                "| {} | {} | {:.0} | {:.1} ms | {:.1} ms | {} | {:.2} | {:.2} | {:.0} MiB |",
                r.watches,
                r.streams,
                r.writes_per_sec,
                r.p50_ms,
                r.p99_ms,
                r.missed,
                r.cost.runs as f64 / writes,
                r.cost.cores(),
                r.rss_mib
            );
        }
        cap_hit = over.err().unwrap_or_else(|| "not refused".to_owned());
        println!(
            "watch {} on one node: {cap_hit}",
            calimero_node::watch::MAX_WATCHES_PER_NODE + 1
        );
    }

    // A fresh cluster per heavy row would be cleaner; the watches above stay registered
    // (streams are held by their tasks), so the rows below run on a second app.
    let app2: String = owner
        .apps()
        .create(typed(json!({ "name": "live2", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .expect("the app");
    let _ = set(&owner, &app2, "warm", "x").await;
    let leader2 = c.app_leader(&app2).expect("a leader");
    let w2 = (leader2 + 1) % 3;
    let writer2 = signed_in(&c, &mut pk, leader2).await;
    let watcher2 = signed_in(&c, &mut pk, w2).await;

    println!("\n## updates/s: every watch dirty all the time (`entries`, 1 s interval)");
    let mut ups = Vec::new();
    for watches in [64, 256, 1_024].into_iter().filter(|_| want("updates")) {
        ups.push(updates(&watcher2, &writer2, &app2, watches, short).await);
    }
    println!("| watches | asked frames/s | delivered frames/s | view runs/s | view run time/s | CPU cores busy | RSS |");
    println!("|---|---|---|---|---|---|---|");
    for u in &ups {
        println!(
            "| {} | {:.0} | {:.0} | {:.0} | {:.2} s | {:.2} | {:.0} MiB |",
            u.watches,
            u.asked_per_sec,
            u.frames_per_sec,
            u.cost.runs as f64 / u.cost.wall,
            u.cost.run_secs / u.cost.wall,
            u.cost.cores(),
            u.rss_mib
        );
    }

    let app3: String = owner
        .apps()
        .create(typed(json!({ "name": "live3", "bundle": &bundle })))
        .await
        .map(|a| a.app_id)
        .expect("the app");
    let _ = set(&owner, &app3, "warm", "x").await;
    let leader3 = c.app_leader(&app3).expect("a leader");
    let w3 = (leader3 + 1) % 3;
    let writer3 = signed_in(&c, &mut pk, leader3).await;
    let watcher3 = signed_in(&c, &mut pk, w3).await;

    println!("\n## same view, many watchers (each watch re-runs the view)");
    let mut fans = Vec::new();
    for n in [1, 10, 100, 1_000].into_iter().filter(|_| want("fanout")) {
        fans.push(fanout(&watcher3, &writer3, &app3, n, 5).await);
    }
    println!("| watchers | view runs per change | shared answers per change | CPU per change | first frame p50 | last frame p50 | last p99 |");
    println!("|---|---|---|---|---|---|---|");
    for f in &fans {
        println!(
            "| {} | {:.1} | {:.1} | {:.1} ms | {:.1} ms | {:.1} ms | {:.1} ms |",
            f.watchers,
            f.runs_per_change,
            f.shared_per_change,
            f.cpu_per_change_ms,
            f.first.0,
            f.last.0,
            f.last.1
        );
    }

    println!("\n## burst: one key written back to back, 64 watchers");
    let b = if want("burst") {
        Some(burst(&watcher3, &writer3, &app3, short).await)
    } else {
        None
    };
    if let Some(b) = &b {
        println!(
        "writes/s {:.0}; frames per watch per s {:.2}; bytes/s to the 64 watchers {:.0}; view runs/s {:.0}; CPU cores busy {:.2}",
        b.writes_per_sec, b.frames_per_watch_per_sec, b.bytes_per_sec,
        b.cost.runs as f64 / b.cost.wall, b.cost.cores()
    );
    }

    println!("\n## system reads: `myRole` watchers while 64 sessions refresh");
    let mut sys = Vec::new();
    for watches in [64, 256, 1_024].into_iter().filter(|_| want("system")) {
        sys.push(system_reads(&c, &watcher3, &mut pk, &app3, watches, short).await);
    }
    println!(
        "| watches | refreshes/s | frames/s | view runs/s | deferred/s | view run time/s | CPU cores busy |"
    );
    println!("|---|---|---|---|---|---|---|");
    for r in &sys {
        println!(
            "| {} | {:.0} | {:.1} | {:.0} | {:.0} | {:.2} s | {:.2} |",
            r.watches,
            r.refreshes_per_sec,
            r.frames_per_sec,
            r.cost.runs as f64 / r.cost.wall,
            r.cost.deferred as f64 / r.cost.wall,
            r.cost.run_secs / r.cost.wall,
            r.cost.cores()
        );
    }

    println!("\n## polling baseline: 256 clients reading one `get`");
    let mut polls = Vec::new();
    for ms in [500, 1_000, 4_000].into_iter().filter(|_| want("poll")) {
        polls.push(
            poll(
                &watcher3,
                &app3,
                256,
                Duration::from_millis(ms),
                short.max(Duration::from_secs(8)),
            )
            .await,
        );
    }
    println!("| interval | clients | requests/s | answer bytes/s | p50 | p99 | view runs/s | CPU cores busy |");
    println!("|---|---|---|---|---|---|---|---|");
    for p in &polls {
        println!(
            "| {} ms | {} | {:.0} | {:.0} | {:.1} ms | {:.1} ms | {:.0} | {:.2} |",
            p.interval_ms,
            p.clients,
            p.requests_per_sec,
            p.bytes_per_sec,
            p.p50_ms,
            p.p99_ms,
            p.cost.runs as f64 / p.cost.wall,
            p.cost.cores()
        );
    }

    if let Some(out) = out {
        let report = json!({
            "bench": "live_views",
            "label": label,
            "nodes": 3,
            "cores": std::thread::available_parallelism().map_or(0, |n| n.get()),
            "secondsPerRow": short.as_secs_f64(),
            "viewRuns": costs.iter().map(|v| json!({
                "view": v.name, "plainUs": v.plain_us, "recordedUs": v.recorded_us,
                "readSetBytes": v.read_set_bytes,
            })).collect::<Vec<_>>(),
            "stages": st.as_ref().map(|st| json!({
                "samples": st.samples,
                "committedMs": [st.commit.0, st.commit.1],
                "appliedOnWatcherMs": [st.applied.0, st.applied.1],
                "frameMs": [st.frame.0, st.frame.1],
            })),
            "capacity": caps.iter().map(|r| json!({
                "watches": r.watches, "streams": r.streams, "writesPerSecond": r.writes_per_sec,
                "p50Ms": r.p50_ms, "p99Ms": r.p99_ms, "missed": r.missed, "rssMib": r.rss_mib,
                "cost": r.cost.json(),
            })).collect::<Vec<_>>(),
            "capRefusal": cap_hit,
            "updates": ups.iter().map(|u| json!({
                "watches": u.watches, "askedPerSecond": u.asked_per_sec,
                "deliveredPerSecond": u.frames_per_sec, "rssMib": u.rss_mib,
                "cost": u.cost.json(),
            })).collect::<Vec<_>>(),
            "fanout": fans.iter().map(|f| json!({
                "watchers": f.watchers, "firstMs": [f.first.0, f.first.1],
                "lastMs": [f.last.0, f.last.1],
                "viewRunsPerChange": f.runs_per_change, "sharedPerChange": f.shared_per_change,
                "cpuMsPerChange": f.cpu_per_change_ms,
            })).collect::<Vec<_>>(),
            "burst": b.as_ref().map(|b| json!({
                "writesPerSecond": b.writes_per_sec,
                "framesPerWatchPerSecond": b.frames_per_watch_per_sec,
                "bytesPerSecond": b.bytes_per_sec,
                "cost": b.cost.json(),
            })),
            "systemReads": sys.iter().map(|r| json!({
                "watches": r.watches, "refreshesPerSecond": r.refreshes_per_sec,
                "framesPerSecond": r.frames_per_sec, "cost": r.cost.json(),
            })).collect::<Vec<_>>(),
            "polling": polls.iter().map(|p| json!({
                "intervalMs": p.interval_ms, "clients": p.clients,
                "requestsPerSecond": p.requests_per_sec, "bytesPerSecond": p.bytes_per_sec,
                "p50Ms": p.p50_ms, "p99Ms": p.p99_ms, "cost": p.cost.json(),
            })).collect::<Vec<_>>(),
        });
        std::fs::write(
            &out,
            serde_json::to_vec_pretty(&report).expect("serializes"),
        )
        .expect("writes the report");
        println!("wrote {out}");
    }
}
