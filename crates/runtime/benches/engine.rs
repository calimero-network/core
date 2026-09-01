//! What does the VM cost, separated into compile and call?
//!
//! `Engine::compile` runs wasmer's full pipeline including gas metering
//! instrumentation, and is paid on every cold path. `Module::run` is the
//! per-call cost. Splitting them answers the only question that changes a
//! decision here: whether caching compiled modules is worth building.
//!
//! The guest is the fixed-work WAT fixture from
//! `crates/runtime/tests/cost_is_flat.rs` — deliberately not a real contract,
//! so a change in the number means a change in the VM rather than in whatever
//! app happened to be compiled that week.
//!
//! Companion, not substitute: `cost_is_flat.rs` asserts that gas and read
//! counts do not grow with store size. That is the gate. This is the clock.

use std::hint::black_box;

use calimero_account::AccountId;
use calimero_runtime::logic::VMLimits;
use calimero_runtime::store::{InMemoryStorage, Storage};
use calimero_runtime::Engine;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};

/// Reads one key `n` times per call; `n` is fixed in the module, so the only
/// variable across the sweep is how many host calls one execution makes.
fn guest_wat(reads: u32) -> String {
    format!(
        r#"
(module
  (import "env" "storage_read" (func $storage_read (param i64 i64) (result i32)))
  (import "env" "read_register" (func $read_register (param i64 i64) (result i32)))
  (memory (export "memory") 2)
  (data (i32.const 0)  "\40\00\00\00\00\00\00\00\01\00\00\00\00\00\00\00")
  (data (i32.const 64) "k")
  (func (export "fixed_reads")
    (local $i i32)
    (local.set $i (i32.const 0))
    (block $done
      (loop $again
        (br_if $done (i32.ge_u (local.get $i) (i32.const {reads})))
        (drop (call $storage_read (i64.const 0) (i64.const 0)))
        (drop (call $read_register (i64.const 0) (i64.const 1024)))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $again))))
)
"#
    )
}

fn engine(c: &mut Criterion) {
    let mut group = c.benchmark_group("engine");

    let wasm = wat::parse_str(guest_wat(64)).expect("the fixture WAT must parse");

    group.bench_function("compile", |b| {
        b.iter(|| {
            black_box(
                Engine::with_limits(VMLimits::default())
                    .compile(black_box(&wasm))
                    .expect("the fixture must compile"),
            )
        });
    });

    for reads in [1_u32, 64, 1_024] {
        let wasm = wat::parse_str(guest_wat(reads)).expect("the fixture WAT must parse");
        let module = Engine::with_limits(VMLimits::default())
            .compile(&wasm)
            .expect("the fixture must compile");

        group.bench_with_input(BenchmarkId::new("run", reads), &module, |b, module| {
            b.iter_batched(
                || {
                    let mut storage = InMemoryStorage::default();
                    storage.set(b"k".to_vec(), vec![7_u8; 64]);
                    storage
                },
                |mut storage| {
                    black_box(
                        module
                            .run(
                                [0_u8; 32].into(),
                                AccountId::from([0_u8; 32]),
                                [0_u8; 32].into(),
                                "fixed_reads",
                                &[],
                                &mut storage,
                                None,
                                None,
                            )
                            .expect("the fixture must run"),
                    )
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

criterion_group!(benches, engine);
criterion_main!(benches);
