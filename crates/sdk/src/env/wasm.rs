//! The guest side of `env`: calls through the `calimero:app` and `wasi` imports.
//!
//! A compute build binds only the compute world, so the calls it lacks come from `denied`.

use crate::bindings::wasi::random::random;

#[cfg_attr(calimero_compute, path = "wasm/denied.rs")]
mod calls;

pub use calls::*;

/// The clock is `std::time`, which reaches `wasi:clocks/wall-clock`: no override here.
pub fn time_now_ms() -> Option<u64> {
    None
}

pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let piece = (len - out.len()).min(calimero_app_wit::MAX_RANDOM_BYTES_PER_CALL);
        out.extend(random::get_random_bytes(piece as u64));
    }
    out
}
