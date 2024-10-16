use std::ops::Deref;
pub use {borsh, serde, serde_json};

pub mod env;
pub mod event;
mod returns;
pub mod state;
mod sys;

pub mod app {
    pub use calimero_sdk_macros::{destroy, emit, event, init, logic, state};
}

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd)]
pub struct Id([u8; 32]);

impl Deref for Id {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}

#[cfg(test)]
mod integration_tests_package_usage {
    use trybuild as _;
}
