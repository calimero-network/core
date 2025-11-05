//! Gossipsub Broadcast Protocols
//!
//! Handlers for one-to-many broadcast messages over gossipsub.
//!
//! These protocols are PUBLIC (anyone can receive broadcasts) but use
//! encryption to protect content (sender_key encryption).

pub mod state_delta;

