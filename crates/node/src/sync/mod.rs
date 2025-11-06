#![expect(
    clippy::mod_module_files,
    reason = "sync module has multiple submodules"
)]

//! Peer synchronization protocols and coordination.
//!
//! This module routes events into two flows:
//! - `broadcast` — gossipsub-driven DAG updates and state delta handling.
//! - `direct` — point-to-point stream protocols (sync, blob share, key share).

mod blobs;
pub mod broadcast;
mod config;
pub mod direct;
mod key;
mod manager;
pub(crate) mod stream;
mod tracking;

pub use config::SyncConfig;
pub use manager::SyncManager;
