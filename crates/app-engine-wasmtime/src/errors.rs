//! How a failed run becomes one `TrapKind` (spec section 5, error mapping).

use calimero_app_host::{HostDenied, TrapKind};
use thiserror::Error;
use wasmtime::Error;
use wasmtime::Trap;
use wasmtime_wasi::I32Exit;

/// A memory or table growth the store's limits refused; raised by the limiter so it
/// traps instead of handing the guest a failed `memory.grow`.
#[derive(Debug, Error)]
#[error("memory or table limit reached")]
pub(crate) struct GrowRefused;

/// A panic in host code, caught at the engine boundary (S6).
#[derive(Debug, Error)]
#[error("host panic")]
pub(crate) struct HostPanic;

/// Wraps a policy refusal so it travels through wasmtime as the trap's root cause.
pub(crate) fn denied(refusal: HostDenied) -> Error {
    Error::new(refusal)
}

/// Classifies a failed run. The detail never carries guest arguments, results or
/// account ids (S10): it is the trap code or a fixed text.
pub(crate) fn classify(err: &Error) -> (TrapKind, String) {
    if let Some(refusal) = err.downcast_ref::<HostDenied>() {
        return (TrapKind::HostDenied, refusal.to_string());
    }
    if err.downcast_ref::<GrowRefused>().is_some() {
        return (TrapKind::OutOfMemory, GrowRefused.to_string());
    }
    if err.downcast_ref::<HostPanic>().is_some() {
        return (TrapKind::Fault, HostPanic.to_string());
    }
    if err.downcast_ref::<I32Exit>().is_some() {
        return (TrapKind::Fault, "guest called exit".to_owned());
    }
    match err.downcast_ref::<Trap>() {
        Some(Trap::OutOfFuel) => (TrapKind::OutOfFuel, Trap::OutOfFuel.to_string()),
        Some(Trap::Interrupt) => (TrapKind::Timeout, Trap::Interrupt.to_string()),
        Some(Trap::StackOverflow) => (TrapKind::StackOverflow, Trap::StackOverflow.to_string()),
        Some(trap) => (TrapKind::Fault, trap.to_string()),
        None => (TrapKind::Fault, "guest fault".to_owned()),
    }
}
