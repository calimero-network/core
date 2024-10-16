pub use {borsh, serde, serde_json};

pub mod env;
pub mod event;
mod returns;
pub mod state;
mod sys;

pub mod app {
    pub use calimero_sdk_macros::{destroy, emit, event, init, logic, state};
}

#[cfg(not(target_arch = "wasm32"))]
#[path = "tests/mocks.rs"]
mod mocks;

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}

#[doc(hidden)]
mod wasm_mocks_package_usage {
    use hex as _;
}

#[cfg(test)]
mod integration_tests_package_usage {
    use trybuild as _;
}
