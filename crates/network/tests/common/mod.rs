//! Shared helpers for `calimero-network` integration tests.
//!
//! The pieces here let a test stand up a controllable libp2p relay server in
//! the same process, so the relay-reservation lifecycle can be exercised
//! deterministically without depending on a deployed boot-node.

pub mod mock_relay;
