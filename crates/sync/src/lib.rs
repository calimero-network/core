//! Calimero Sync Crate
//!
//! **NO ACTORS** - Clean async Rust orchestration
//!
//! This crate orchestrates synchronization between Calimero nodes using
//! the stateless protocols from `calimero-protocols`.
//!
//! # Architecture
//!
//! ```text
//! SyncScheduler (orchestration)
//!     ↓
//! Strategies (dag_catchup, state_resync)
//!     ↓
//! Protocols (stateless functions)
//!     ↓
//! Network (libp2p streams)
//! ```
//!
//! # Key Differences from Old Architecture
//!
//! **Old (SyncManager)**:
//! - Actor-based (Actix)
//! - Message passing
//! - Tight coupling
//! - Hard to test
//!
//! **New (SyncScheduler)**:
//! - Plain async Rust
//! - Event-driven
//! - Protocol composition
//! - Easy to test
//!
//! # Example
//!
//! ```rust,ignore
//! use calimero_sync::{SyncScheduler, SyncConfig};
//!
//! // Create scheduler (NO actors!)
//! let scheduler = SyncScheduler::new(
//!     node_client,
//!     context_client,
//!     network_client,
//!     config,
//! );
//!
//! // Start sync for a context (plain async!)
//! scheduler.sync_context(&context_id, &peer_id).await?;
//! ```

pub mod config;
pub mod events;
pub mod scheduler;
pub mod strategies;

// Re-export main types
pub use config::SyncConfig;
pub use events::{SyncEvent, SyncStatus};
pub use scheduler::SyncScheduler;
pub use strategies::{SyncStrategy, SyncResult};

