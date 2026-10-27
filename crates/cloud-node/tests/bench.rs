//! G1 measurements (spec D32, plan Task 13 Step 1) on the in-process three-node cluster.
//! Ignored by default; run in release on an otherwise idle machine:
//!
//! `cargo test --release -p calimero-cloud-node --test bench -- --ignored --nocapture --test-threads 1`
//!
//! Each measurement prints one `MEASURE` line. Engine-level numbers (fuel, engine time,
//! fresh instance, spin) invoke the node's own engine on the leader's applied state with
//! the node's own directory, because the API does not return an outcome's usage.

mod common;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    Budget, Caller, Engine, HostCore, Invocation, InvocationKind, Outcome, Prepared, Status,
    TrapKind,
};
use calimero_cloud_client::{Client, Consistency};
use calimero_cloud_node::app::SysDirectory;
use calimero_cloud_node::Running;
use common::{signed_up, Cluster};
use serde_json::{json, Value};

const SLOT_MS: u64 = 900_000;

fn pct(sorted: &[Duration], p: f64) -> f64 {
    let i = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[i].as_secs_f64() * 1e3
}

fn summary(name: &str, mut d: Vec<Duration>) {
    d.sort();
    println!(
        "MEASURE {name}: n={} p50={:.3}ms p99={:.3}ms min={:.3}ms max={:.3}ms",
        d.len(),
        pct(&d, 0.50),
        pct(&d, 0.99),
        pct(&d, 0.0),
        pct(&d, 1.0),
    );
}

/// Invokes `method` of `app` on node `r`'s engine against its applied state.
fn invoke(
    r: &Running,
    prepared: &Prepared,
    app: &str,
    (kind, method, args): (InvocationKind, &str, &Value),
    caller: Caller,
    budget: Budget,
) -> Outcome {
    let g = r.node.hosted(app).expect("hosted");
    let (state, applied) = g.read_local(|m, applied| (m.view(), applied));
    let dir = Arc::new(SysDirectory::new(Arc::downgrade(&r.node), app.to_owned()));
    let inv = Invocation {
        kind,
        method: method.to_owned(),
        args: serde_json::to_vec(args).unwrap(),
        caller,
        entry_ts_ms: calimero_cloud_node::types::now_ms(),
        id_term: 1_000_000,
        id_seq: 1,
        seed: [7; 32],
        timer: None,
    };
    let host = HostCore::new(app, state, applied, inv, dir).unwrap();
    r.node.app_engine().invoke(prepared, host, budget)
}

async fn account_id(c: &Client) -> String {
    c.me().await.unwrap()["accountId"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "measurement for the G1 report; run by hand in release"]
async fn g1_measurements() {
    let c = Cluster::start(3).await;
    let (owner, mut pk) = signed_up(&c, 0, "owner").await;
    // The owner signed in on every node: clients[i] talks to node i.
    let owner = Arc::new(owner);
    let mut clients = vec![Arc::clone(&owner)];
    for i in 1..3 {
        let cl = c.client(i);
        let _s = cl.sign_in(&mut pk).await.unwrap();
        clients.push(Arc::new(cl));
    }
    let owner_id = account_id(&owner).await;
    let org = owner.post("orgs", json!({ "name": "Acme" })).await.unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned();
    let rooms = common::rooms();
    println!("MEASURE rooms component size: {} bytes", rooms.len());
    let bundle = owner.upload_bundle(rooms.clone()).await.unwrap();
    let app = owner
        .create_app("rooms", &bundle, json!({ "org": org }))
        .await
        .unwrap();
    for room in ["r1", "r2", "lat", "bulk"] {
        let _r = owner
            .call(
                &app,
                "addRoom",
                json!({ "id": room, "name": room, "capacity": 8 }),
                None,
            )
            .await
            .unwrap();
    }
    let leader = c.app_leader(&app).expect("leader");
    let lr = c.nodes[leader].as_ref().unwrap();
    let prepared = lr.node.prepared_for(&bundle).await.unwrap();
    let caller = || Caller::Account(owner_id.clone());
    let far = (calimero_cloud_node::types::now_ms() / SLOT_MS + 1_000) * SLOT_MS;

    // 1. book: fuel and engine time (instantiate + handle, wall) on the leader's state.
    for slots in [1u32, 2] {
        let args =
            json!({ "roomId": "r1", "start": far.to_string(), "slotCount": slots, "title": "t" });
        let mut fuel = Vec::new();
        let mut times = Vec::new();
        for _ in 0..1_000 {
            let o = invoke(
                lr,
                &prepared,
                &app,
                (InvocationKind::Call, "book", &args),
                caller(),
                Budget::call(),
            );
            assert_eq!(o.status, Status::Ok, "{:?}", o.status);
            fuel.push(o.usage.fuel);
            times.push(o.usage.cpu);
        }
        fuel.sort_unstable();
        println!(
            "MEASURE book fuel ({slots} slot(s), empty room): min={} max={}",
            fuel[0],
            fuel[fuel.len() - 1]
        );
        summary(&format!("book engine time ({slots} slot(s))"), times);
    }

    // 2. call latency over 1,000 sequential books through the API, each a fresh slot.
    for (label, node) in [("leader", leader), ("follower", (leader + 1) % 3)] {
        let cl = &clients[node];
        let room = if label == "leader" { "lat" } else { "r2" };
        let mut d = Vec::new();
        for k in 0..1_000u64 {
            let start = far + k * SLOT_MS;
            let args =
                json!({ "roomId": room, "start": start.to_string(), "slotCount": 1, "title": "t" });
            let t = Instant::now();
            let _r = cl
                .call(&app, "book", args, Some(&format!("lat-{label}-{k}")))
                .await
                .unwrap();
            d.push(t.elapsed());
        }
        summary(&format!("book call latency via {label} (node {node})"), d);
    }

    // 2b. view latency on the leader, to separate the leadership check (a strong view
    // confirms leadership like a call does) from the append and commit.
    for (label, consistency) in [
        ("strong", Consistency::Strong),
        ("stale", Consistency::Stale { min_index: 0 }),
    ] {
        let cl = &clients[leader];
        let mut d = Vec::new();
        for _ in 0..1_000 {
            let t = Instant::now();
            let _r = cl
                .view(&app, "booking", json!({ "id": "none" }), consistency)
                .await;
            d.push(t.elapsed());
        }
        summary(&format!("{label} view latency via leader"), d);
    }

    // 3. sustained writes per second on one app log: `conc` books in flight across the
    // three nodes for 10 s each.
    let next = Arc::new(AtomicU64::new(0));
    for conc in [1usize, 8, 32, 128] {
        let before = c.applied_on(leader, &app);
        let ok = Arc::new(AtomicU64::new(0));
        let failed = Arc::new(AtomicU64::new(0));
        let deadline = Instant::now() + Duration::from_secs(10);
        let t = Instant::now();
        let tasks: Vec<_> = (0..conc)
            .map(|w| {
                let cl = Arc::clone(&clients[w % 3]);
                let (app, next, ok, failed) = (
                    app.clone(),
                    Arc::clone(&next),
                    Arc::clone(&ok),
                    Arc::clone(&failed),
                );
                tokio::spawn(async move {
                    while Instant::now() < deadline {
                        let k = next.fetch_add(1, Ordering::Relaxed);
                        let start = far + 10_000 * SLOT_MS + k * SLOT_MS;
                        let args = json!({ "roomId": "bulk", "start": start.to_string(), "slotCount": 1, "title": "t" });
                        match cl.call(&app, "book", args, Some(&format!("w-{k}"))).await {
                            Ok(_) => ok.fetch_add(1, Ordering::Relaxed),
                            Err(_) => failed.fetch_add(1, Ordering::Relaxed),
                        };
                    }
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let secs = t.elapsed().as_secs_f64();
        let entries = c.applied_on(leader, &app) - before;
        println!(
            "MEASURE sustained books, {conc} in flight: ok={} failed={} in {secs:.2}s = {:.0} acked writes/s; log entries {entries} = {:.0}/s",
            ok.load(Ordering::Relaxed),
            failed.load(Ordering::Relaxed),
            ok.load(Ordering::Relaxed) as f64 / secs,
            entries as f64 / secs,
        );
    }

    // 4. fresh instance: the kv fixture's `len` view (no reads but one counter).
    let kv = common::component("kv");
    let kv_bundle = owner.upload_bundle(kv.clone()).await.unwrap();
    let kv_app = owner.create_app("kv", &kv_bundle, json!({})).await.unwrap();
    let kv_leader = c.app_leader(&kv_app).expect("kv leader");
    let kr = c.nodes[kv_leader].as_ref().unwrap();
    let kv_prepared = kr.node.prepared_for(&kv_bundle).await.unwrap();
    let mut d = Vec::new();
    let mut fuel = 0;
    for _ in 0..1_000 {
        let o = invoke(
            kr,
            &kv_prepared,
            &kv_app,
            (InvocationKind::View, "len", &json!({})),
            caller(),
            Budget::view(),
        );
        assert_eq!(o.status, Status::Ok, "{:?}", o.status);
        fuel = o.usage.fuel;
        d.push(o.usage.cpu);
    }
    println!("MEASURE kv len fuel: {fuel}");
    summary("fresh instance + trivial view (kv len)", d);

    // 5. spin under the default budget (fuel 200,000,000, wall clock 2 s).
    let b = Budget::call();
    for _ in 0..5 {
        let t = Instant::now();
        let o = invoke(
            kr,
            &kv_prepared,
            &kv_app,
            (InvocationKind::Call, "spin", &json!({})),
            caller(),
            b,
        );
        let wall = t.elapsed();
        let kind = match &o.status {
            Status::Trap { kind, .. } => *kind,
            other => panic!("spin ended {other:?}"),
        };
        assert_eq!(kind, TrapKind::OutOfFuel);
        println!(
            "MEASURE spin default budget: {kind:?} fuel={} engine={:.1}ms wall={:.1}ms ns/unit={:.3}",
            o.usage.fuel,
            o.usage.cpu.as_secs_f64() * 1e3,
            wall.as_secs_f64() * 1e3,
            o.usage.cpu.as_nanos() as f64 / o.usage.fuel as f64,
        );
    }
    let r = owner
        .call(&kv_app, "spin", json!({}), Some("spin-api"))
        .await;
    println!("MEASURE spin through the API: {r:?}");

    // 6. Rooms cold compile (a fresh engine and cache dir each time) and a cache hit.
    for _ in 0..3 {
        let dir = tempfile::tempdir().unwrap();
        let engine = WasmtimeEngine::new(dir.path().to_owned(), &[1; 32]).unwrap();
        let t = Instant::now();
        let _p = engine.prepare("rooms", &rooms).unwrap();
        let cold = t.elapsed();
        let again = WasmtimeEngine::new(dir.path().to_owned(), &[1; 32]).unwrap();
        let t = Instant::now();
        let _p = again.prepare("rooms", &rooms).unwrap();
        let warm = t.elapsed();
        let cached: u64 = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        println!(
            "MEASURE rooms prepare: cold={:.1}ms from cache={:.1}ms cached artifact={cached} bytes",
            cold.as_secs_f64() * 1e3,
            warm.as_secs_f64() * 1e3
        );
    }
}
