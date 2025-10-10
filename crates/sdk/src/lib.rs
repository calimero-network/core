// Re-export the embed_abi macro from wasm-abi
pub use calimero_sdk_macros::CallbackHandlers;
pub use calimero_wasm_abi::embed_abi;
pub use {borsh, serde, serde_json};

pub mod env;
pub mod event;
mod macros;
pub mod private_storage;
mod returns;
pub mod state;
pub mod types;

pub mod app {
    use super::types::Error;

    pub type Result<T, E = Error> = core::result::Result<T, E>;

    pub use calimero_sdk_macros::{
        bail, callbacks, destroy, emit, err, event, init, log, logic, private, state,
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
