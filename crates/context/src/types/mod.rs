//! Common types used across the context crate

pub mod atomic;
pub mod repr;

pub use atomic::*;
pub use repr::*;

use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};

// Re-export commonly used types
pub use calimero_primitives::context::{Context, ContextId, ContextInvitationPayload};
pub use calimero_primitives::application::{Application, ApplicationId};
pub use calimero_primitives::identity::{PrivateKey, PublicKey};
pub use calimero_primitives::hash::Hash;

// Context-specific types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct SignerId(pub [u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ProposalId(pub [u8; 32]);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct ContextIdentity(pub [u8; 32]);

// Do not redefine `Application`; use the one from calimero_primitives

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, Serialize, Deserialize)]
#[expect(clippy::exhaustive_enums, reason = "Considered to be exhaustive")]
pub enum Capability {
    ManageApplication,
    ManageMembers,
    Proxy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct Revision(pub u64);
