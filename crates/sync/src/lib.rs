//! State synchronization orchestration for distributed nodes.
//!
//! Provides sync strategies (DAG catchup, full resync) with retry logic and observability.
//! Uses protocols from [`calimero_protocols`] to fetch and apply missing deltas.
//!
//! # Example
//!
//! ```rust,ignore
//! use calimero_sync::strategies::{DagCatchup, SyncStrategy};
//!
//! let strategy = DagCatchup::new(network_client, context_client, timeout);
//! let result = strategy.execute(&context_id, &peer_id, &our_identity, &delta_store).await?;
//! ```

#![warn(missing_docs)]

pub mod config;
pub mod events;
pub mod scheduler;
pub mod strategies;

pub use config::{RetryConfig, SyncConfig};
pub use events::{SyncEvent, SyncStatus};
pub use scheduler::SyncScheduler;
pub use strategies::{SyncResult, SyncStrategy};
