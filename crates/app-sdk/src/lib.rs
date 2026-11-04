//! The old name of [`calimero-sdk`](https://github.com/calimero-network/core/tree/master/crates/sdk),
//! kept while apps move over.
//!
//! This crate re-exports `calimero-sdk` and nothing else, so `use calimero_app_sdk::prelude::*;`
//! and every `calimero_app_sdk::...` path mean what they mean in `calimero-sdk`. The `app`
//! attributes expand to paths through whichever of the two names an app depends on.
//!
//! Depend on `calimero-sdk` instead (`calimero-sdk = { .. }`, `use calimero_sdk::prelude::*;`):
//! this crate is deleted once no app uses it.

pub use calimero_sdk::*;
