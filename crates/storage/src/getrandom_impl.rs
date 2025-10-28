//! Custom getrandom implementation for Calimero.
//!
//! This provides a getrandom backend that uses Calimero's existing
//! `env::random_bytes()` instead of requiring the "js" feature.
//!
//! This eliminates the need for additional WASM dependencies and ensures
//! all randomness comes from Calimero's host-provided random source.

use getrandom::Error;

/// Custom getrandom implementation using Calimero's random infrastructure.
///
/// This function is called by the `getrandom` crate (and transitively by `uhlc`)
/// when the "custom" feature is enabled.
///
/// # Safety
///
/// This function is marked unsafe because it's called from C-like FFI context.
/// However, our implementation is safe - it just calls Calimero's random_bytes.
#[allow(unsafe_code, clippy::missing_safety_doc)]
#[no_mangle]
unsafe extern "Rust" fn __getrandom_v03_custom(dest: *mut u8, len: usize) -> Result<(), Error> {
    if dest.is_null() {
        return Err(Error::UNSUPPORTED);
    }

    // Convert raw pointer to slice
    #[expect(unsafe_code, reason = "Required for getrandom custom backend")]
    let buf = unsafe { core::slice::from_raw_parts_mut(dest, len) };

    // Use Calimero's existing random infrastructure
    crate::env::random_bytes(buf);

    Ok(())
}
