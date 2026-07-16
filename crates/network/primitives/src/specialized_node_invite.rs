//! Specialized Node Types
//!
//! This module defines the specialized-node classification used by the fleet
//! TEE admission path (`TeeAttestationAnnounce`).

use borsh::{BorshDeserialize, BorshSerialize};

/// Type of specialized node
#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum SpecializedNodeType {
    /// Read-only node - receives state updates but cannot execute transactions
    ReadOnly,
}
