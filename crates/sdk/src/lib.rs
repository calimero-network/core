pub use borsh;
pub use serde;
pub use serde_json;

pub mod env;
mod returns;
mod sys;

pub mod app {
    pub use calimero_sdk_macros::{destroy, event, logic, state};
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};

    pub mod marker {
        use crate::borsh::{BorshDeserialize, BorshSerialize};

        pub trait AppState: Default + BorshSerialize + BorshDeserialize {}
    }
}
