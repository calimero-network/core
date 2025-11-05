//! Domain services for node operations.
//!
//! This module contains focused services that handle specific aspects of node functionality:
//! - `timer_manager`: Periodic task scheduling and execution
//! - `delta_applier`: Delta application to WASM storage

pub mod delta_applier;
pub mod timer_manager;

pub use delta_applier::ContextDeltaApplier;
pub use timer_manager::TimerManager;
