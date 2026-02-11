//! Sync Protocol Simulation Framework
//!
//! A deterministic, event-driven simulation framework for testing the Calimero
//! sync protocol under realistic distributed system conditions.
//!
//! # Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                         Test Suite                               │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!                               ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        SimRuntime                                │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐              │
//! │  │  SimClock   │  │ EventQueue  │  │ ChaCha8Rng  │              │
//! │  │  (logical)  │  │ (time,seq)  │  │  (seeded)   │              │
//! │  └─────────────┘  └─────────────┘  └─────────────┘              │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!                               ▼
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                        SimNetwork                                │
//! │  - Message routing with delivery scheduling                     │
//! │  - Fault injection (latency, loss, reorder, duplicate)          │
//! │  - Partition modeling (connectivity cuts)                       │
//! └─────────────────────────────────────────────────────────────────┘
//!                               │
//!               ┌───────────────┼───────────────┐
//!               ▼               ▼               ▼
//!        ┌──────────┐    ┌──────────┐    ┌──────────┐
//!        │ SimNode  │    │ SimNode  │    │ SimNode  │
//!        │ "alice"  │    │  "bob"   │    │"charlie" │
//!        └──────────┘    └──────────┘    └──────────┘
//! ```
//!
//! # Quick Start
//!
//! ```rust,ignore
//! use sync_sim::prelude::*;
//!
//! // Create runtime with seed for reproducibility
//! let mut rt = SimRuntime::new(42);
//!
//! // Add nodes
//! let alice = rt.add_node("alice");
//! let bob = rt.add_node("bob");
//!
//! // Set up initial state
//! rt.node_mut(&alice).unwrap().insert_entity(
//!     EntityId::from_u64(1),
//!     vec![1, 2, 3],
//!     CrdtType::LwwRegister,
//! );
//!
//! // Run until convergence
//! let converged = rt.run_until_converged();
//! assert!(converged);
//! ```
//!
//! # Features
//!
//! - **Deterministic execution**: Same seed = same results
//! - **Event-driven**: Process events in (time, seq) order
//! - **Fault injection**: Latency, loss, reorder, duplicates
//! - **Partition modeling**: Network splits and heals
//! - **Crash/restart**: Node failure and recovery
//! - **Convergence checking**: Formal properties (C1-C5)
//! - **Metrics collection**: Protocol cost, work done, effects

pub mod actions;
#[macro_use]
pub mod assertions;
pub mod convergence;
pub mod digest;
pub mod metrics;
pub mod network;
pub mod node;
pub mod runtime;
pub mod scenarios;
pub mod sim_runtime;
pub mod types;

/// Prelude for convenient imports.
pub mod prelude {
    pub use super::actions::{
        EntityMetadata, EntityTransfer, OutgoingMessage, SelectedProtocol, StorageOp, SyncActions,
        SyncMessage, TimerOp,
    };
    pub use super::assertions::{
        all_converged, divergence_percentage, majority_digest, nodes_converged,
    };
    pub use super::convergence::{
        check_convergence, is_deadlocked, ConvergenceInput, ConvergencePending,
        ConvergenceProperty, ConvergenceResult,
    };
    pub use super::digest::{compute_state_digest, DigestCache, DigestEntity};
    pub use super::metrics::{
        ConvergenceMetrics, EffectMetrics, NodeMetrics, ProtocolMetrics, SimMetrics, WorkMetrics,
    };
    pub use super::network::{
        FaultConfig, NetworkRouter, PartitionManager, PartitionSpec, SimEvent,
    };
    pub use super::node::{SimNode, SyncState};
    pub use super::runtime::{EventQueue, EventSeq, SimClock, SimDuration, SimRng, SimTime};
    pub use super::scenarios::{RandomScenario, Scenario};
    pub use super::sim_runtime::{SimConfig, SimRuntime, StopCondition};
    pub use super::types::{DeltaId, EntityId, MessageId, NodeId, StateDigest, TimerId, TimerKind};

    // Note: assertion macros (assert_converged!, assert_entity_count!, etc.) are available
    // via `#[macro_use]` on the assertions module and don't need explicit re-export.
}

// Re-export for external use
pub use prelude::*;
