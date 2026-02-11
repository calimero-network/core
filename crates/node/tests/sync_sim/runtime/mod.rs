//! Simulation runtime components.
//!
//! Provides the core infrastructure for deterministic event-driven simulation:
//! - `SimClock` - Logical time progression
//! - `EventQueue` - Priority queue with (time, seq) ordering
//! - `SimRng` - Deterministic random number generation

pub mod clock;
pub mod queue;
pub mod rng;

pub use clock::{SimClock, SimDuration, SimTime};
pub use queue::{EventQueue, EventSeq};
pub use rng::SimRng;
