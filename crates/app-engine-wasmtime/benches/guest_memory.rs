//! What a call costs in guest code that is mostly linear-memory loads and stores, and
//! what the instance pool costs to create and to recycle a slot.
//!
//! The replica engine's memory reservation decides whether cranelift emits a bounds
//! check on every load and store (a reservation under 4 GiB) or relies on the guard
//! region (4 GiB plus a guard). `tests/guests/crunch` sorts 1 Mi integers, sieves
//! primes below 8 Mi and multiplies two 192 x 192 matrices; `noop` returns at once,
//! so it is the fixed cost of a call: instantiating from a pool slot and resetting it.
//!
//! Before the criterion groups it prints the engine's start-up time (creating the
//! pool) and the process's virtual size and resident set before and after, so the
//! address space the pool reserves is measured, not estimated.
//!
//!     cargo bench -p calimero-app-engine-wasmtime --bench guest_memory

#![allow(
    clippy::unwrap_used,
    clippy::print_stdout,
    reason = "a bench's panic is its report, and it prints its measurements"
)]

use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use calimero_app_build::build_in_repo_cache;
use calimero_app_engine_wasmtime::WasmtimeEngine;
use calimero_app_host::{
    Budget, Caller, Directory, Engine, HostCore, Invocation, InvocationKind, OrgRole, Prepared,
    Status,
};
use criterion::{criterion_group, Criterion};

const CACHE_KEY: [u8; 32] = [5; 32];
const METHODS: [&str; 4] = ["noop", "sort", "sieve", "matmul"];

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

fn host(method: &str) -> HostCore {
    let inv = Invocation {
        kind: InvocationKind::Call,
        method: method.to_owned(),
        args: Vec::new(),
        caller: Caller::Account("alice".to_owned()),
        entry_ts_ms: 1_700_000_000_123,
        id_term: 1,
        id_seq: 1,
        seed: [7; 32],
        timer: None,
        compute: None,
    };
    let state: BTreeMap<Vec<u8>, Vec<u8>> = BTreeMap::new();
    HostCore::new("app-1", Arc::new(state), 1, inv, Arc::new(NoDirectory)).unwrap()
}

/// Enough fuel and time for the heaviest method; memory is a call's default (64 MiB).
fn budget() -> Budget {
    Budget {
        fuel: 50_000_000_000,
        wall_clock: Duration::from_secs(60),
        ..Budget::call()
    }
}

fn call(engine: &WasmtimeEngine, app: &Prepared, method: &str) -> Vec<u8> {
    let out = engine.invoke(app, host(method), budget());
    assert!(
        matches!(out.status, Status::Ok),
        "{method}: {:?}",
        out.status
    );
    out.result
}

/// `VmSize` and `VmRSS` of this process, in KiB (Linux only).
fn vm() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| {
        status
            .lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
    };
    Some((field("VmSize:")?, field("VmRSS:")?))
}

fn show_vm(label: &str) {
    match vm() {
        Some((size, rss)) => println!(
            "{label:<28} VmSize {:>10.1} GiB   VmRSS {:>8.1} MiB",
            size as f64 / (1024.0 * 1024.0),
            rss as f64 / 1024.0
        ),
        None => println!("{label:<28} (no /proc/self/status)"),
    }
}

fn engine(cache: &Path) -> WasmtimeEngine {
    WasmtimeEngine::new(cache.to_path_buf(), &CACHE_KEY).unwrap()
}

/// Start-up time and address space of the pool, printed once.
fn report_pool(component: &[u8]) {
    show_vm("before the engine");
    let mut starts = Vec::new();
    for _ in 0..5 {
        let cache = tempfile::tempdir().unwrap();
        let start = Instant::now();
        let e = engine(cache.path());
        starts.push(start.elapsed());
        drop(e);
    }
    starts.sort();
    println!(
        "engine start (pool created)  min {:?}   median {:?}",
        starts[0], starts[2]
    );
    let cache = tempfile::tempdir().unwrap();
    let e = engine(cache.path());
    show_vm("engine up, no call yet");
    let app = e.prepare("crunch", component).unwrap();
    for method in METHODS {
        let start = Instant::now();
        let result = call(&e, &app, method);
        println!(
            "{method:<8} first call {:>10.2?}  checksum {}",
            start.elapsed(),
            hex(&result)
        );
    }
    show_vm("after one call of each");
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn guest_memory(c: &mut Criterion) {
    let component = component();
    let cache = tempfile::tempdir().unwrap();
    let e = engine(cache.path());
    let app = e.prepare("crunch", &component).unwrap();
    let mut group = c.benchmark_group("guest_memory");
    let _ = group.sample_size(20);
    for method in METHODS {
        let _ = group.bench_function(method, |b| {
            b.iter(|| black_box(call(&e, &app, method)));
        });
    }
    group.finish();
}

fn component() -> Vec<u8> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/crunch");
    build_in_repo_cache(&dir).unwrap().app.component
}

criterion_group!(benches, guest_memory);

fn main() {
    report_pool(&component());
    benches();
    Criterion::default().configure_from_args().final_summary();
}
