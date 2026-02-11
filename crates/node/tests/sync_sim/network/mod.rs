//! Network simulation components.
//!
//! Provides message routing, fault injection, and partition modeling.

pub mod faults;
pub mod partition;
pub mod router;

pub use faults::FaultConfig;
pub use partition::{PartitionManager, PartitionSpec};
pub use router::{InFlightMessage, NetworkMetrics, NetworkRouter, SimEvent};
