//! The `app` attributes and the types they implement.
//!
//! ```ignore
//! use calimero_app_sdk::prelude::*;
//!
//! #[app::state(version = 1)]
//! pub struct Rooms { bookings: Table<Booking> }
//!
//! #[app::error]
//! pub enum Error { Taken { start: u64 } }
//!
//! #[app::logic]
//! impl Rooms {
//!     #[app::init]
//!     pub fn init() -> app::Result<Self> { Ok(Self::load()) }
//!
//!     pub fn book(&mut self, start: u64) -> app::Result<()> { app::bail!(Error::Taken { start }) }
//! }
//! ```
//!
//! `#[app::error]` and `#[app::event]` must sit in the same module as `#[app::logic]` (or be
//! imported there by their `__CALIMERO_ABI_*` constants), because that is where the ABI is
//! assembled.

pub use calimero_app_sdk_macros::{
    allow, captcha, confirm, error, event, every, init, logic, public, state, view,
};

pub use crate::__app_bail as bail;
pub use crate::error::Result;

/// The state root. Implemented by `#[app::state]`.
pub trait AppState: Sized {
    /// The schema version (`#[app::state(version = N)]`), used by migrations.
    const VERSION: u32;
    /// The `"state"` part of the ABI.
    const ABI: &'static str;

    /// The state: every field is a table or collection handle, so this reads nothing.
    fn load() -> Self;
}

/// A table or collection that can be a field of the app state; `#[app::state]` lists each
/// field's [`StateField::KIND`] under `"fields"` in the ABI's `"state"`.
pub trait StateField {
    /// The kind of field, in camelCase: `table`, `unorderedMap`, `counter`, `cell`, ..
    const KIND: &'static str;
    /// The fields the host indexes for full-text search, as a JSON list: a table's
    /// `#[table(search = [..])]`, empty for everything else.
    const SEARCH: &'static str = "[]";
}

/// An event enum. Implemented by `#[app::event]`.
pub trait AppEvent {
    /// The event name (the variant name) and its JSON data.
    fn encode(&self) -> Result<(&'static str, serde_json::Value)>;
}

/// Return early with an app error: `app::bail!(Error::Taken { start })`.
#[doc(hidden)]
#[macro_export]
macro_rules! __app_bail {
    ($error:expr) => {
        return ::core::result::Result::Err(::core::convert::Into::<$crate::AppError>::into($error))
    };
}
