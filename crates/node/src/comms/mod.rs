//! Communication protocol implementations shared by the node.
//!
//! - `broadcast` covers gossipsub-based messaging.
//! - `direct` covers point-to-point libp2p streams.

pub mod broadcast;
pub mod direct;
