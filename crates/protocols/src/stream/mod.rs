//! Authenticated Stream Utilities
//!
//! Provides authenticated, encrypted P2P stream communication.
//!
//! # Security Model
//!
//! ALL P2P streams MUST use `SecureStream` (ported from node/sync/):
//! - Challenge-response authentication (prevents impersonation)
//! - Mutual identity verification
//! - Encrypted message passing (SharedKey encryption)
//! - Nonce rotation (prevents replay attacks)
//!
//! # Design
//!
//! This module contains the ported SecureStream from node crate.
//! Eventually will be refactored to simpler AuthenticatedStream API.

mod authenticated;
mod helpers;
mod tracking;

pub use authenticated::SecureStream;
pub use tracking::{Sequencer, SyncProtocol, SyncState};

// Re-export helpers for use within protocols crate only
pub(crate) use helpers::{recv, send};
