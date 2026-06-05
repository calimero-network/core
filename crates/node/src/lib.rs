//! Calimero node orchestration and coordination.
//!
//! **Purpose**: Main node runtime that coordinates sync, storage, networking, and event handling.
//! **Key Components**:
//! - `NodeManager`: Main actor coordinating all services
//! - `NodeClients`: External service clients (context, node)
//! - `NodeManagers`: Service managers (blobstore, sync)
//! - `NodeState`: Runtime state (caches)

#![allow(clippy::print_stdout, reason = "Acceptable for CLI")]
#![allow(
    clippy::multiple_inherent_impl,
    reason = "TODO: Check if this is necessary"
)]

mod arbiter_pool;
mod constants;
pub mod dag_compactor;
mod delta_store;
pub mod gc;
pub mod handlers;
pub mod join_namespace;
pub mod key_delivery;
mod manager;
pub mod migration_status;
pub mod network_event_channel;
pub mod network_event_processor;
mod node_metrics;
mod peer_identity_cache;
mod peer_identity_persist;
pub mod readiness;
mod run;
mod specialized_node_invite_state;
mod state;
pub(crate) mod state_delta_bridge;
pub mod sync;
pub(crate) mod sync_session_bridge;
mod utils;

pub use manager::NodeManager;
pub use network_event_channel::{
    channel as network_event_channel, NetworkEventChannelConfig, NetworkEventSender,
};
pub use network_event_processor::NetworkEventBridge;
pub use run::{start, NodeConfig, NodeMode, SpecializedNodeConfig};
pub(crate) use state::{CachedBlob, NodeClients, NodeManagers, NodeState};
pub use sync::SyncManager;

#[cfg(test)]
mod cascade_dispatch_e2e;
#[cfg(test)]
mod delta_store_batch_test;
#[cfg(test)]
mod delta_store_head_hashes_test;
#[cfg(test)]
mod local_governance_node_e2e;
