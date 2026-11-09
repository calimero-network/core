//! How a failed run becomes one `TrapKind` (spec section 5, error mapping).

use calimero_app_host::{HostDenied, TrapKind};
use thiserror::Error;
use wasmtime::component::ResourceTableError;
use wasmtime::Error;
use wasmtime::PoolConcurrencyLimitError;
use wasmtime::Trap;
use wasmtime_wasi::I32Exit;

/// The detail of a run refused because the node has no instance slot free (Ruling 22).
pub(crate) const POOL_EXHAUSTED: &str = "instance pool exhausted";
/// The detail of a run refused because the node could not allocate its instance.
pub(crate) const ALLOCATION_FAILED: &str = "instance allocation failed";
/// What wasmtime says when a store's instance, memory or table count is exceeded.
const COUNT_LIMIT_PREFIX: &str = "resource limit exceeded";

/// A memory or table growth the store's limits refused; raised by the limiter so it
/// traps instead of handing the guest a failed `memory.grow`.
#[derive(Debug, Error)]
#[error("memory or table limit reached")]
pub(crate) struct GrowRefused;

/// The detail of a run that used its whole wall-clock budget.
pub const WALL_CLOCK_EXCEEDED: &str = "wall clock exceeded";
/// The detail of a memory growth the operating system refused.
pub(crate) const MEMORY_UNAVAILABLE: &str = "memory allocation failed";
/// The detail of a run ended by its interrupt flag.
pub const CANCELLED: &str = "cancelled";

/// The interrupt flag of a run was set; raised from the epoch callback.
#[derive(Debug, Error)]
#[error("cancelled")]
pub(crate) struct Cancelled;

/// A memory growth the limiter allowed but the operating system refused: the node's problem,
/// not the guest's.
#[derive(Debug, Error)]
#[error("memory allocation failed")]
pub(crate) struct GrowFailed;

/// A panic in host code, caught at the engine boundary (S6).
#[derive(Debug, Error)]
#[error("host panic")]
pub(crate) struct HostPanic;

/// Where a run failed: building its instance, or running `handle`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Instantiate,
    Call,
}

/// Wraps a policy refusal so it travels through wasmtime as the trap's root cause.
pub(crate) fn denied(refusal: HostDenied) -> Error {
    Error::new(refusal)
}

/// Classifies a failed run. The detail never carries guest arguments, results or
/// account ids (S10): it is the trap code or a fixed text.
///
/// Node load is not the guest's fault: an instance pool with no free slot, or an
/// allocation the OS refused while instantiating or growing memory, is `Unavailable` (not
/// logged, retried with the same key; D23, D36).
pub(crate) fn classify(err: &Error, phase: Phase) -> (TrapKind, String) {
    if let Some(refusal) = err.downcast_ref::<HostDenied>() {
        return (TrapKind::HostDenied, refusal.to_string());
    }
    if let Some(full @ ResourceTableError::Full) = err.downcast_ref::<ResourceTableError>() {
        // The per-store resource cap (`MAX_RESOURCES`) is a host limit, like MAX_WRITES.
        return (TrapKind::HostDenied, full.to_string());
    }
    if err.downcast_ref::<GrowRefused>().is_some() {
        return (TrapKind::OutOfMemory, GrowRefused.to_string());
    }
    if err.downcast_ref::<Cancelled>().is_some() {
        return (TrapKind::Cancelled, CANCELLED.to_owned());
    }
    if err.downcast_ref::<GrowFailed>().is_some() {
        return (TrapKind::Unavailable, MEMORY_UNAVAILABLE.to_owned());
    }
    if err.downcast_ref::<HostPanic>().is_some() {
        return (TrapKind::Fault, HostPanic.to_string());
    }
    if err.downcast_ref::<I32Exit>().is_some() {
        return (TrapKind::Fault, "guest called exit".to_owned());
    }
    if err.downcast_ref::<PoolConcurrencyLimitError>().is_some() {
        return (TrapKind::Unavailable, POOL_EXHAUSTED.to_owned());
    }
    match err.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => return (TrapKind::OutOfFuel, Trap::OutOfFuel.to_string()),
        Some(Trap::Interrupt) => return (TrapKind::Timeout, WALL_CLOCK_EXCEEDED.to_owned()),
        Some(Trap::StackOverflow) => {
            return (TrapKind::StackOverflow, Trap::StackOverflow.to_string())
        }
        Some(trap) => return (TrapKind::Fault, trap.to_string()),
        None => {}
    }
    if phase == Phase::Instantiate {
        if err.downcast_ref::<std::io::Error>().is_some() {
            return (TrapKind::Unavailable, ALLOCATION_FAILED.to_owned());
        }
        if err.to_string().starts_with(COUNT_LIMIT_PREFIX) {
            return (
                TrapKind::OutOfMemory,
                "instance, memory or table count limit".to_owned(),
            );
        }
    }
    (TrapKind::Fault, "guest fault".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(err: Error, phase: Phase) -> (TrapKind, String) {
        classify(&err, phase)
    }

    #[test]
    fn node_side_failures_are_unavailable_and_the_guests_own_are_not() {
        assert_eq!(
            kind(Error::new(GrowFailed), Phase::Call),
            (TrapKind::Unavailable, MEMORY_UNAVAILABLE.to_owned())
        );
        assert_eq!(
            kind(Error::new(GrowRefused), Phase::Call),
            (TrapKind::OutOfMemory, GrowRefused.to_string())
        );
        let io = Error::new(std::io::Error::other("mmap"));
        assert_eq!(
            kind(io, Phase::Instantiate),
            (TrapKind::Unavailable, ALLOCATION_FAILED.to_owned())
        );
    }

    #[test]
    fn cancel_and_wall_clock_are_distinct() {
        assert_eq!(
            kind(Error::new(Cancelled), Phase::Call),
            (TrapKind::Cancelled, CANCELLED.to_owned())
        );
        assert_eq!(
            kind(Error::new(Trap::Interrupt), Phase::Call),
            (TrapKind::Timeout, WALL_CLOCK_EXCEEDED.to_owned())
        );
    }
}
