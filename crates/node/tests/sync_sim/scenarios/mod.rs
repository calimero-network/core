//! Test scenario builders.
//!
//! See spec §15 - Protocol Negotiation Tests.

pub mod buffering;
pub mod deterministic;
pub mod hash_comparison;
pub mod levelwise;
pub mod nested_container;
pub mod random;

pub use deterministic::Scenario;
pub use random::RandomScenario;
