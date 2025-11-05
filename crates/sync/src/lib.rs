//! Calimero Sync - Clean async orchestration
//!
//! **NO ACTORS** - Plain async Rust sync orchestration
//!
//! This crate orchestrates synchronization between Calimero nodes using
//! the stateless protocols from [`calimero_protocols`].
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
//! # Key Components
//!
//! - [`SyncScheduler`]: Main orchestration component
//!   - Tracks active syncs
//!   - Executes strategies with retry logic
//!   - Emits sync events for observability
//!   - Manages periodic heartbeat (optional)
//!
//! - [`SyncStrategy`]: Strategy trait for sync approaches
//!   - [`strategies::DagCatchup`]: Fetch missing deltas (most common)
//!   - [`strategies::StateResync`]: Full state resync (fallback)
//!
//! - [`SyncEvent`]: Event-driven observability
//!   - Started, Completed, Failed events
//!   - Duration tracking
//!   - Retry attempt tracking
//!
//! - [`SyncConfig`]: Configuration
//!   - Timeout, retry, heartbeat settings
//!   - Exponential backoff parameters
//!
//! # Example Usage
//!
//! ```rust,ignore
//! use calimero_sync::{SyncScheduler, SyncConfig};
//! use calimero_sync::strategies::DagCatchup;
//!
//! // Create scheduler (NO actors!)
//! let scheduler = SyncScheduler::new(
//!     node_client,
//!     context_client,
//!     network_client,
//!     SyncConfig::default(),
//! );
//!
//! // Create strategy
//! let strategy = DagCatchup::new(
//!     network_client,
//!     context_client,
//!     timeout,
//! );
//!
//! // Sync a context (plain async!)
//! let result = scheduler.sync_context(
//!     &context_id,
//!     &peer_id,
//!     &our_identity,
//!     &delta_store,
//!     &strategy,
//! ).await?;
//! ```
//!
//! # Testing
//!
//! - 10 comprehensive tests covering config, events, and scheduler
//! - No infrastructure needed - pure async tests
//! - See `tests/` directory for examples

#![warn(missing_docs)]

pub mod config;
pub mod events;
pub mod scheduler;
pub mod strategies;

// Re-export main types
pub use config::{RetryConfig, SyncConfig};
pub use events::{SyncEvent, SyncStatus};
pub use scheduler::SyncScheduler;
pub use strategies::{SyncResult, SyncStrategy};
