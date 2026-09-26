//! Gas for a logical operation must not depend on node-local derived state, or
//! one replica lands a write another traps on. A forward tripwire: it starts
//! failing when a cache or derived index is added to the write path.

use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

mod wall_harness;

use wall_harness::{call, guest_wasm};

fn expect_ok(outcome: &Outcome, method: &str) {
    assert!(
        outcome.returns.is_ok(),
        "{method} failed: {:?}",
        outcome.returns
    );
}

/// One shared seed: rebuilding it per branch would put timestamp noise into the prior state.
fn seeded_module_and_storage() -> (calimero_runtime::Module, InMemoryStorage) {
    let wasm = guest_wasm("collaborative-editor");
    let module = Engine::with_limits(VMLimits::default())
        .compile(&wasm)
        .expect("compile metered module");

    let mut storage = InMemoryStorage::default();
    expect_ok(
        &call(&module, &mut storage, "init", &serde_json::json!({})),
        "init",
    );
    expect_ok(
        &call(
            &module,
            &mut storage,
            "insert_text",
            &serde_json::json!({"position": 0_usize, "text": "hello world"}),
        ),
        "insert_text (seed)",
    );
    (module, storage)
}

/// One `insert_text` against the shared seed; `cold` skips the reads that would warm a cache.
fn measure_insert_gas(
    module: &calimero_runtime::Module,
    storage: &InMemoryStorage,
    cold: bool,
) -> u64 {
    let mut storage = storage.clone();

    if !cold {
        for _ in 0..5 {
            expect_ok(
                &call(module, &mut storage, "get_text", &serde_json::json!({})),
                "get_text (warm-up)",
            );
            expect_ok(
                &call(module, &mut storage, "get_stats", &serde_json::json!({})),
                "get_stats (warm-up)",
            );
            expect_ok(
                &call(module, &mut storage, "get_length", &serde_json::json!({})),
                "get_length (warm-up)",
            );
        }
    }

    let outcome = call(
        module,
        &mut storage,
        "insert_text",
        &serde_json::json!({"position": 5_usize, "text": "!"}),
    );
    expect_ok(&outcome, "insert_text (measured)");
    outcome
        .gas_used
        .expect("a successful outcome always reports gas_used")
}

/// A tolerance, not `assert_eq!`: each call draws a fresh HLC timestamp that moves gas.
const TOLERANCE_FRACTION: f64 = 0.01;

#[test]
fn insert_gas_is_independent_of_local_derived_state() {
    let (module, storage) = seeded_module_and_storage();
    let cold = measure_insert_gas(&module, &storage, /* node-local caches empty */ true);
    let warm = measure_insert_gas(
        &module, &storage, /* node-local caches populated */ false,
    );
    println!("cold gas: {cold}");
    println!("warm gas: {warm}");

    let diff = cold.abs_diff(warm);
    #[expect(
        clippy::cast_precision_loss,
        reason = "gas values are well under f64's exact-integer range; a\
                   tolerance check does not need bit-exact precision"
    )]
    let allowed = (cold.max(warm) as f64 * TOLERANCE_FRACTION) as u64;
    assert!(
        diff <= allowed,
        "insert consumed {cold} gas cold and {warm} gas warm (diff {diff}, allowed \
         {allowed}); a gas difference this large is far beyond the wall-clock-timestamp \
         noise floor this test tolerates, and means node-local state now affects insert \
         gas: two replicas can disagree about whether a write happened"
    );
}
