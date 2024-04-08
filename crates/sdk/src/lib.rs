pub mod env;
mod returns;
mod sys;

pub mod app {
    pub use calimero_sdk_macros::{destroy, event, logic, state};
}

#[doc(hidden)]
pub mod __private {
    pub use crate::returns::{IntoResult, WrappedReturn};
}
