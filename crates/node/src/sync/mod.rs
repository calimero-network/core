#![expect(
    clippy::mod_module_files,
    reason = "sync module has multiple submodules"
)]

//! Peer synchronization orchestration.
//!
//! Protocol implementations live under `crate::comms`; this module keeps the
//! higher-level scheduling and state tracking.

mod config;
mod manager;
pub(crate) mod stream;
mod tracking;

pub use config::SyncConfig;
pub use manager::SyncManager;
pub(crate) use tracking::{Sequencer, SyncProtocol};
