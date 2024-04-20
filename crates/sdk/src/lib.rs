pub use borsh;
pub use serde;
pub use serde_json;

pub mod env;
pub mod event;
mod returns;
mod sys;

pub mod app {
    pub use calimero_sdk_macros::{destroy, emit, event, logic, state};
}

pub mod marker {
    use crate::borsh::{BorshDeserialize, BorshSerialize};
    use crate::serde::Serialize;

    pub trait AppState: Default + BorshSerialize + BorshDeserialize {
        type Event<'a>: AppEvent + 'a;
    }

    pub trait AppEvent: Serialize {}
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}
