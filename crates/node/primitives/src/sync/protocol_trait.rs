//! Common trait for sync protocol implementations.
//!
//! This module defines the [`SyncProtocolExecutor`] trait that all sync protocols
//! implement. This enables:
//!
//! - Protocol implementation details contained within each protocol module
//! - Common interface for `SyncManager` to invoke any protocol
//! - Same code path for production and simulation (only `Store` backend differs)
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                     SyncProtocolExecutor trait                   │
//! │  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐ │
//! │  │ HashComparison  │  │    Snapshot     │  │   BloomFilter   │ │
//! │  │    Protocol     │  │    Protocol     │  │    Protocol     │ │
//! │  └────────┬────────┘  └────────┬────────┘  └────────┬────────┘ │
//! │           │                    │                    │          │
//! │           └────────────────────┼────────────────────┘          │
//! │                                │                               │
//! │                    ┌───────────┴───────────┐                   │
//! │                    │   SyncTransport       │                   │
//! │                    │ (Stream or SimStream) │                   │
//! │                    └───────────────────────┘                   │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use calimero_node_primitives::sync::{SyncProtocolExecutor, HashComparisonProtocol};
//!
//! // Production
//! let mut transport = StreamTransport::new(&mut stream);
//! let stats = HashComparisonProtocol::run_initiator(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//!     HashComparisonConfig { remote_root_hash },
//! ).await?;
//!
//! // Simulation (exact same call, different transport/store)
//! let mut transport = SimStream::new();
//! let stats = HashComparisonProtocol::run_initiator(
//!     &mut transport,
//!     &store,  // Store<InMemoryDB>
//!     context_id,
//!     identity,
//!     HashComparisonConfig { remote_root_hash },
//! ).await?;
//! ```

use async_trait::async_trait;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_store::Store;
use eyre::Result;

use super::SyncTransport;

/// Trait for sync protocol implementations.
///
/// Each sync protocol (HashComparison, Snapshot, BloomFilter, etc.) implements
/// this trait. The protocol logic is generic over:
///
/// - `T: SyncTransport` - the transport layer (production streams or simulation channels)
/// - `Store` - the storage backend (RocksDB or InMemoryDB)
///
/// This enables the same protocol code to run in both production and simulation.
///
/// Note: Uses `?Send` because `RuntimeEnv` (used for storage access) contains `Rc`
/// which is not `Send`. Callers must not spawn these futures across threads.
#[async_trait(?Send)]
pub trait SyncProtocolExecutor {
    /// Protocol-specific configuration for the initiator.
    ///
    /// For example, HashComparison needs the remote root hash.
    type Config: Send;

    /// Protocol-specific statistics/results.
    type Stats: Send + Default;

    /// Run the initiator (pulling) side of the protocol.
    ///
    /// The initiator requests data from the responder and applies it locally.
    ///
    /// # Arguments
    ///
    /// * `transport` - The transport for sending/receiving messages
    /// * `store` - The local storage (works with both RocksDB and InMemoryDB)
    /// * `context_id` - The context being synced
    /// * `identity` - Our identity for this context
    /// * `config` - Protocol-specific configuration
    ///
    /// # Returns
    ///
    /// Protocol-specific statistics on success.
    async fn run_initiator<T: SyncTransport>(
        transport: &mut T,
        store: &Store,
        context_id: ContextId,
        identity: PublicKey,
        config: Self::Config,
    ) -> Result<Self::Stats>;

    /// Run the responder side of the protocol.
    ///
    /// The responder answers requests from the initiator.
    ///
    /// # Arguments
    ///
    /// * `transport` - The transport for sending/receiving messages
    /// * `store` - The local storage
    /// * `context_id` - The context being synced
    /// * `identity` - Our identity for this context
    async fn run_responder<T: SyncTransport>(
        transport: &mut T,
        store: &Store,
        context_id: ContextId,
        identity: PublicKey,
    ) -> Result<()>;
}
