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
//! │  │ HashComparison  │  │    Snapshot     │  │   LevelWise     │ │
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
//! # Responder Dispatch Model
//!
//! The `SyncManager` dispatches incoming sync requests using this flow:
//!
//! 1. Manager receives stream and calls `recv()` to get the first message
//! 2. Manager matches on `InitPayload` to determine which protocol to use
//! 3. Manager extracts protocol-specific data from the first message
//! 4. Manager calls `run_responder()` passing the extracted data via `ResponderInit`
//!
//! This design is necessary because the manager must peek at the first message
//! for routing, but once consumed it cannot be "un-read". The `ResponderInit`
//! associated type allows each protocol to declare what data it needs from
//! the first request.
//!
//! # Example
//!
//! ```ignore
//! use calimero_node_primitives::sync::{SyncProtocolExecutor, HashComparisonProtocol};
//!
//! // Production initiator
//! let mut transport = StreamTransport::new(&mut stream);
//! let stats = HashComparisonProtocol::run_initiator(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//!     HashComparisonConfig { remote_root_hash },
//! ).await?;
//!
//! // Production responder (manager extracts first request data)
//! let first_request = HashComparisonFirstRequest { node_id, max_depth };
//! HashComparisonProtocol::run_responder(
//!     &mut transport,
//!     &store,
//!     context_id,
//!     identity,
//!     first_request,
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
/// Each sync protocol (HashComparison, Snapshot, LevelWise, etc.) implements
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

    /// Data extracted from the first request for responder dispatch.
    ///
    /// The manager parses the first `InitPayload` and constructs this type
    /// to pass to `run_responder`. This is necessary because the manager
    /// consumes the first message for routing, so the protocol cannot
    /// `recv()` it again.
    ///
    /// For example:
    /// - HashComparison needs `{ node_id, max_depth }` from `TreeNodeRequest`
    /// - LevelWise needs `{ level, parent_ids }` from `LevelWiseRequest`
    type ResponderInit: Send;

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
    /// The responder answers requests from the initiator. The first request's
    /// data is passed via `first_request` because the manager has already
    /// consumed the first message for routing.
    ///
    /// # Arguments
    ///
    /// * `transport` - The transport for sending/receiving messages
    /// * `store` - The local storage
    /// * `context_id` - The context being synced
    /// * `identity` - Our identity for this context
    /// * `first_request` - Data from the first `InitPayload`, extracted by the manager
    async fn run_responder<T: SyncTransport>(
        transport: &mut T,
        store: &Store,
        context_id: ContextId,
        identity: PublicKey,
        first_request: Self::ResponderInit,
    ) -> Result<()>;
}
