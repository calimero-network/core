//! New Node Runtime - NO ACTORS!
//!
//! Clean async Rust runtime that replaces the actor-based architecture.
//!
//! # Architecture
//!
//! ```text
//! Event Loop (tokio::select!)
//!     ↓
//! Protocol Dispatch (stateless protocols)
//!     ↓
//! Sync Orchestration (calimero-sync)
//! ```
//!
//! # Key Components
//!
//! - **Event Loop**: Main async loop handling all events
//! - **Protocol Dispatch**: Routes messages to stateless protocols
//! - **Network Listeners**: Spawn tasks for gossipsub and P2P
//! - **Periodic Tasks**: Heartbeat, cleanup, etc.
//!
//! # Example
//!
//! ```rust,ignore
//! use calimero_node::runtime::NodeRuntime;
//!
//! // Create runtime (NO actors!)
//! let runtime = NodeRuntime::new(
//!     node_client,
//!     context_client,
//!     network_client,
//!     config,
//! );
//!
//! // Run (plain async!)
//! runtime.run().await?;
//! ```

pub mod dispatch;
pub mod event_loop;
pub mod listeners;
pub mod tasks;

// Re-export main types
pub use event_loop::NodeRuntime;

