//! Calimero SDK for building CRDT-based distributed applications.
//!
//! # Event Handlers ⚠️
//!
//! **IMPORTANT**: Event handlers may execute in **parallel** (not guaranteed sequential).
//!
//! Your handlers MUST be:
//! - **Commutative**: Order-independent (use CRDTs)
//! - **Independent**: No shared mutable state
//! - **Idempotent**: Safe to retry
//! - **Pure**: Only modify CRDT state, no external side effects
//!
//! See [`event`] module documentation for detailed requirements and examples.

// Note: embed_abi macro is deprecated - use JSON files instead
// pub use calimero_wasm_abi::embed_abi;
pub use {borsh, serde, serde_json};

pub mod env;
pub mod event;
mod macros;
pub mod private_storage;
mod returns;
pub mod state;
pub mod types;
pub use calimero_primitives::identity::PublicKey;

pub mod app {
    use super::types::Error;

    pub type Result<T, E = Error> = core::result::Result<T, E>;

    pub use calimero_sdk_macros::{
        bail, destroy, emit, err, event, init, log, logic, private, state,
    };
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}

#[cfg(test)]
mod integration_tests_package_usage {
    use trybuild as _;
}
