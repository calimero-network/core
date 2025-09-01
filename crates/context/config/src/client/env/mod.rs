//! Environment configurations for different client contexts.
//!
//! This module provides environment-specific configurations organized by protocol.

pub mod config;
pub mod proxy;

// Re-export the ContextConfig trait
pub use config::ContextConfig;
