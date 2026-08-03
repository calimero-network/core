//! The ABI manifest an app publishes, and how it arrives at one.
//!
//! [`abi_type`] is the sole producer: `#[app::logic]` builds the manifest from
//! the `AbiType` impls the app's types carry, so the compiler resolves aliases,
//! macro-generated and re-exported types before anything is described.

pub mod abi_type;
pub mod downgrade;
pub mod embed;
pub mod manifest_builder;
pub mod schema;
pub mod validate;

pub use abi_type::{AbiType, TypeRegistry};
pub use manifest_builder::ManifestBuilder;
pub use schema::*;
pub use validate::*;
