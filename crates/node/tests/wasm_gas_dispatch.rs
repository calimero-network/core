//! Gas metering through the node's execution dispatch primitive.
//!
//! Context execution runs guest WASM via
//! `global_runtime().spawn_blocking(move || module.run_with_origin(..))`
//! (`crates/context/src/handlers/execute/mod.rs`). The runtime's own unit tests
//! prove a runaway guest traps with `GasExhausted` at the `Module::run` level;
//! this test proves the property survives the *dispatch* layer — that a guest
//! calling no host function (so no host limit applies) does not pin a thread on
//! the node's blocking pool forever, but returns and frees the thread.
//!
//! The test uses the exact primitive the node dispatches on
//! (`calimero_utils_actix::global_runtime().spawn_blocking`). Without metering
//! the guest below loops forever, the join handle never resolves, and the
//! watchdog `timeout` fires the test — which is precisely the node-thread
//! starvation the meter exists to prevent.

use std::time::Duration;

use calimero_runtime::errors::FunctionCallError;
use calimero_runtime::logic::VMLimits;
use calimero_runtime::store::InMemoryStorage;
use calimero_runtime::Engine;
use calimero_utils_actix::{global_runtime, init_global_runtime};

/// A guest whose only export loops forever with no host call — the canonical
/// tight loop that escapes every limit except gas.
const SPIN_FOREVER_WAT: &str = r#"
    (module
        (memory (export "memory") 1)
        (func (export "spin_forever")
            (loop $again (br $again))))
"#;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runaway_guest_on_blocking_pool_is_gas_bounded_and_frees_the_thread() {
    // The node initializes this once at startup; a dedicated test binary means
    // this is the first and only initialization, so it succeeds.
    init_global_runtime().expect("init global runtime");

    let wasm = wat::parse_str(SPIN_FOREVER_WAT).expect("parse WAT");
    // A small budget so exhaustion is near-instant; the watchdog below is
    // generous so a genuine hang (metering defeated) is unambiguous.
    let module = Engine::with_limits(VMLimits {
        max_gas: 200_000,
        ..Default::default()
    })
    .compile(&wasm)
    .expect("compile metered module");

    // Dispatch exactly as context execution does: on the global runtime's
    // blocking pool. `None` node client / private storage is fine — the guest
    // never calls a host function.
    let handle = global_runtime().spawn_blocking(move || {
        let mut storage = InMemoryStorage::default();
        module.run(
            [0u8; 32].into(),
            [0u8; 32].into(),
            "spin_forever",
            &[],
            &mut storage,
            None,
            None,
        )
    });

    let outcome = tokio::time::timeout(Duration::from_secs(30), handle)
        .await
        .expect("runaway guest must not pin the blocking thread forever")
        .expect("blocking task must not panic")
        .expect("run must return an Outcome");

    assert!(
        matches!(outcome.returns, Err(FunctionCallError::GasExhausted { .. })),
        "runaway guest dispatched on the blocking pool must trap with GasExhausted, got: {:?}",
        outcome.returns
    );
    assert_eq!(
        outcome.gas_used,
        Some(200_000),
        "an exhausted run reports the full budget as consumed"
    );

    // The blocking thread the runaway used must have been released: a fresh
    // blocking task submitted afterwards runs to completion. Were the guest
    // still spinning (metering defeated), the first `timeout` above would have
    // already failed the test — this is the belt-and-suspenders check that the
    // pool is not left wedged.
    let followup = global_runtime().spawn_blocking(|| 21_u32 * 2);
    let value = tokio::time::timeout(Duration::from_secs(5), followup)
        .await
        .expect("blocking pool must still accept work after a runaway execution")
        .expect("follow-up blocking task must not panic");
    assert_eq!(
        value, 42,
        "the blocking pool must not be wedged by the runaway guest"
    );
}
