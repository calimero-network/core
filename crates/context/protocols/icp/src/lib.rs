//! ICP integration for Calimero context configuration.
//!
//! This crate provides ICP-specific implementations for the Calimero context configuration system.

pub mod query;
pub mod types;

// Re-export the main types for convenience
pub use query::*;
pub use types::*;
