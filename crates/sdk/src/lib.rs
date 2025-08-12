pub use {borsh, serde, serde_json};

// Re-export the embed_abi macro from wasm-abi-v1
pub use calimero_wasm_abi_v1::embed_abi;

pub mod env;
pub mod event;
mod macros;
mod returns;
pub mod state;
mod sys;
pub mod types;

pub mod app {
    use super::types::Error;

    pub type Result<T, E = Error> = core::result::Result<T, E>;

    pub use calimero_sdk_macros::{bail, destroy, emit, err, event, init, log, logic, state, abi_type};
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}

#[cfg(test)]
mod integration_tests_package_usage {
    use trybuild as _;
}
