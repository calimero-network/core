//! Common test utilities for protocol testing
//!
//! This module provides mocks and test fixtures for all protocols.

pub mod mocks;

use calimero_crypto::SharedKey;
use calimero_primitives::context::ContextId;
use calimero_primitives::identity::{PrivateKey, PublicKey};
use rand::{thread_rng, Rng};
use std::num::NonZeroU128;

/// Create a test context ID
pub fn create_test_context_id() -> ContextId {
    ContextId::from(thread_rng().gen::<[u8; 32]>())
}

/// Create a test identity (public key)
pub fn create_test_identity() -> PublicKey {
    let private_key = PrivateKey::random(&mut thread_rng());
    private_key.public_key()
}

/// Create a shared key for testing
pub fn create_test_shared_key() -> SharedKey {
    let private_key = PrivateKey::random(&mut thread_rng());
    let peer_public_key = create_test_identity();
    SharedKey::new(&private_key, &peer_public_key)
}

/// Create a test delta with random ID
pub fn create_test_delta(
    parents: Vec<[u8; 32]>,
) -> calimero_dag::CausalDelta<Vec<calimero_storage::interface::Action>> {
    use calimero_storage::logical_clock::{HybridTimestamp, Timestamp, ID, NTP64};
    
    let delta_id = thread_rng().gen::<[u8; 32]>();
    
    // Create a simple timestamp for testing
    let ntp64 = NTP64(0);
    // ID requires NonZero<u128>
    let id_value = thread_rng().gen_range(1..u128::MAX);
    let id = ID::from(NonZeroU128::new(id_value).unwrap());
    let timestamp = Timestamp::new(ntp64, id);
    let hlc = HybridTimestamp::new(timestamp);
    
    calimero_dag::CausalDelta {
        id: delta_id,
        parents,
        payload: vec![], // Empty actions for testing
        hlc,
        expected_root_hash: [0; 32],
    }
}

