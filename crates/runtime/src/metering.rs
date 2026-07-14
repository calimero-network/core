//! WASM execution gas metering.
//!
//! Guest WASM is untrusted. Every resource limit the runtime enforces is
//! checked *inside a host call*, so a guest that never calls a host function is
//! unbounded: the canonical tight loop (`loop {}`) never returns, never traps,
//! and touches no host limit. It pins the OS thread running it forever, and
//! because executions run on a bounded blocking pool, enough such calls starve
//! the whole node.
//!
//! Gas metering closes that hole deterministically. The Wasmer metering
//! middleware instruments every compiled module with a decrementing points
//! counter, charged at each *accounting point* (branch targets and sources,
//! calls); when the counter would go negative the guest traps. The charge is a
//! pure function of the operators actually executed — [`gas_cost`] — so it is
//! identical on every node. Two nodes running the same method on the same state
//! consume the same gas and agree on whether it ran out. A wall-clock timeout
//! could not offer that: it would let one node time out while another finishes,
//! forking replicated state. Determinism is why this is gas and not a deadline.
//!
//! The counter is baked into the module at compile time (so it survives
//! serialization and works even under a headless engine), but its *starting
//! value* is set per execution from [`VMLimits::max_gas`](crate::logic::VMLimits::max_gas)
//! via [`set_remaining_points`](wasmer_middlewares::metering::set_remaining_points),
//! and exhaustion is detected afterwards via
//! [`get_remaining_points`](wasmer_middlewares::metering::get_remaining_points).

use wasmer::wasmparser::Operator;
use wasmer::Instance;
use wasmer::{AsStoreMut, AsStoreRef};

/// Name of the exported global the metering middleware injects to hold the
/// remaining points. Present on every module we compile (the middleware always
/// injects it); its absence means a module was produced without metering, which
/// we treat as "unmetered" rather than panicking.
const REMAINING_POINTS_GLOBAL: &str = "wasmer_metering_remaining_points";

/// Gas charged per executed WASM operator.
///
/// A flat one-point-per-operator model: a run's total gas is, to first order,
/// the number of operators it executes. Uniform cost keeps the meter trivial to
/// reason about and — the property that actually matters — *stable*. Every node
/// must charge identically or their execution outcomes diverge, so the model is
/// deliberately simple and fixed in code. Treat any change to it as
/// consensus-affecting, exactly like a change to the default budget.
///
/// The signature matches what
/// [`Metering::new`](wasmer_middlewares::Metering::new) expects
/// (`Fn(&Operator) -> u64 + Send + Sync + 'static`); a plain `fn` item
/// satisfies it.
pub fn gas_cost(_operator: &Operator<'_>) -> u64 {
    1
}

/// Whether `instance` was compiled with the metering middleware.
///
/// The middleware's own `get`/`set_remaining_points` helpers *panic* on a
/// module that lacks the injected globals. Every module the runtime compiles is
/// metered, so that should never happen — but the runtime also accepts
/// precompiled artifacts through a separate path, and a defensive check here
/// keeps a hypothetical unmetered module from turning a missing-global lookup
/// into a panic (which would only be caught by the outer `catch_unwind` and
/// surfaced as a confusing guest panic). When this returns `false` the caller
/// simply skips metering for that run.
pub(crate) fn is_metered(store: &impl AsStoreRef, instance: &Instance) -> bool {
    // `get_global` does not need `&mut`, but taking a store ref keeps the two
    // metering helpers' call shapes consistent at the call site.
    let _ = store;
    instance.exports.get_global(REMAINING_POINTS_GLOBAL).is_ok()
}

/// Set the starting gas budget for `instance` to `limit`.
///
/// Thin wrapper over the middleware helper, gated on [`is_metered`] so it is a
/// no-op (never a panic) for an unmetered module.
pub(crate) fn set_gas_limit(store: &mut impl AsStoreMut, instance: &Instance, limit: u64) {
    if is_metered(store, instance) {
        wasmer_middlewares::metering::set_remaining_points(store, instance, limit);
    }
}

/// Whether `instance` has exhausted its gas budget.
///
/// Returns `false` for an unmetered module (nothing to exhaust). A `true`
/// result is only meaningful after the guest has trapped: exhaustion is what
/// *caused* the trap, so the caller inspects this to reclassify an otherwise
/// generic `unreachable` trap as [`FunctionCallError::GasExhausted`].
///
/// [`FunctionCallError::GasExhausted`]: crate::errors::FunctionCallError::GasExhausted
pub(crate) fn is_exhausted(store: &mut impl AsStoreMut, instance: &Instance) -> bool {
    use wasmer_middlewares::metering::MeteringPoints;

    if !is_metered(store, instance) {
        return false;
    }
    matches!(
        wasmer_middlewares::metering::get_remaining_points(store, instance),
        MeteringPoints::Exhausted
    )
}

/// Gas consumed by `instance` given the `budget` it started with, or `None`
/// for an unmetered module. On exhaustion the whole budget was consumed, so
/// this returns `budget`; otherwise `budget - remaining`. Used for the
/// `Outcome::gas_used` telemetry that operators size `max_gas` from.
pub(crate) fn gas_used(
    store: &mut impl AsStoreMut,
    instance: &Instance,
    budget: u64,
) -> Option<u64> {
    use wasmer_middlewares::metering::MeteringPoints;

    if !is_metered(store, instance) {
        return None;
    }
    match wasmer_middlewares::metering::get_remaining_points(store, instance) {
        MeteringPoints::Remaining(points) => Some(budget.saturating_sub(points)),
        MeteringPoints::Exhausted => Some(budget),
    }
}
