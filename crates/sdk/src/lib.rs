pub use {borsh, serde, serde_json};

pub mod env;
pub mod event;
mod returns;
pub mod state;
mod sys;
pub mod types;

use core::result::Result as CoreResult;

pub type Result<T> = CoreResult<T, types::Error>;

pub mod app {
    pub use calimero_sdk_macros::{destroy, emit, event, init, logic, state};
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}

#[cfg(test)]
mod integration_tests_package_usage {
    use trybuild as _;
}
