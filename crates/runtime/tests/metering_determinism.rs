//! Gas for a given logical operation must not depend on node-local derived
//! state: a replica with warm caches would land a call inside `max_gas` where
//! a cold one traps with `GasExhausted`, and two replicas would then disagree
//! about whether the write happened. That is silent divergence, not slowness.
//!
//! It passes today because the write path holds no node-local derived state -
//! `ReplicatedGrowableArray::insert_str` re-derives everything by linearising
//! the stored chars on every call. That is the point: it is a forward
//! tripwire, and it starts failing the moment a cache or derived index is
//! added to the write path, which is the same moment cross-replica gas
//! divergence becomes possible. Do not delete it for looking trivial.
//!
//! The comparison is a tolerance rather than `assert_eq!` because every
//! `insert_str` draws a fresh HLC timestamp whose physical-time component
//! reads the wall clock, so the new `CharId`'s bytes differ between any two
//! runs and move the byte length of a couple of reads (never the entry count),
//! and with it gas. Two structurally-identical cold runs were measured
//! disagreeing by up to 0.18%, and `TOLERANCE_FRACTION` is 5x that; a caching
//! regression shifts the per-call cost by orders more, so it cannot hide in
//! the band.
//!
//! It drives the real `apps/collaborative-editor` guest so the property is
//! checked against the actual `insert_text` path rather than a stand-in, and
//! both branches clone one shared seeded build so the seed itself cannot carry
//! the timestamp noise above.

use calimero_runtime::logic::{Outcome, VMLimits};
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;

mod wall_harness;

use wall_harness::{call, guest_wasm};

fn editor_wasm() -> Vec<u8> {
    guest_wasm("collaborative-editor")
}

fn expect_ok(outcome: &Outcome, method: &str) {
    assert!(
        outcome.returns.is_ok(),
        "{method} failed: {:?}",
        outcome.returns
    );
}

/// Compile the guest once and build ONE seeded document, so both branches of
/// [`measure_insert_gas`] start from byte-identical prior state: rebuilding
/// the seed per branch would put wall-clock-timestamp noise into content that
/// is supposed to be identical either way.
fn seeded_module_and_storage() -> (calimero_runtime::Module, InMemoryStorage) {
    let wasm = editor_wasm();
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

/// Measure ONE `insert_text` call against the shared seed, differing only in
/// whether the document was read several times first. The measured insert is
/// otherwise identical: same prior document, position and text.
fn measure_insert_gas(
    module: &calimero_runtime::Module,
    storage: &InMemoryStorage,
    cold: bool,
) -> u64 {
    let mut storage = storage.clone();

    if !cold {
        // The activity that would populate a node-local cache or derived
        // index, if the write path had one to consult.
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

/// 5x the worst wall-clock-driven gas noise measured cold-vs-cold (0.18%).
const TOLERANCE_FRACTION: f64 = 0.01;

/// The same logical operation must consume the same gas regardless of
/// node-local derived state, or one replica completes a write while another
/// traps on GasExhausted. See the module doc for why an equal result today is
/// expected rather than vacuous, and why the comparison has a tolerance.
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
