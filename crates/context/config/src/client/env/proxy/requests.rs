//! Request types for context proxy operations.
//!
//! This module contains all the request structs used for both querying and mutating
//! context proxy data across different blockchain protocols. These request types
//! are shared between the query and mutate clients to ensure consistency.

use candid::CandidType;
use serde::{Deserialize, Serialize};

use crate::repr::Repr;
use crate::types::{ProposalId, SignerId};
use crate::{Proposal, ProposalAction, ProposalApprovalWithSigner};

// ============================================================================
// Query Request Types
// ============================================================================

/// Request to get the number of active proposals in the context.
///
/// This is a simple request that doesn't require any parameters.
#[derive(Copy, Clone, Debug, Serialize, CandidType)]
pub struct ActiveProposalRequest;

/// Request to retrieve paginated context storage entries.
///
/// # Fields
///
/// * `offset` - The number of entries to skip from the beginning
/// * `limit` - The maximum number of entries to return
#[derive(Clone, Debug, Serialize, CandidType)]
pub struct ContextStorageEntriesRequest {
    /// The number of entries to skip from the beginning
    pub offset: usize,
    /// The maximum number of entries to return
    pub limit: usize,
}

/// Request to retrieve a context variable value by its key.
///
/// # Fields
///
/// * `key` - The byte array key identifying the context variable
#[derive(Clone, Debug, Serialize, CandidType)]
pub struct ContextVariableRequest {
    /// The byte array key identifying the context variable
    pub key: Vec<u8>,
}

/// Request to get approval information for a specific proposal.
///
/// # Fields
///
/// * `proposal_id` - The unique identifier of the proposal
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProposalApprovalsRequest {
    /// The unique identifier of the proposal
    pub proposal_id: Repr<ProposalId>,
}

/// Request to get the list of identities that have approved a proposal.
///
/// # Fields
///
/// * `proposal_id` - The unique identifier of the proposal
#[derive(Clone, Debug, Serialize)]
pub struct ProposalApproversRequest {
    /// The unique identifier of the proposal
    pub proposal_id: Repr<ProposalId>,
}

/// Request to retrieve a specific proposal by its ID.
///
/// # Fields
///
/// * `proposal_id` - The unique identifier of the proposal to retrieve
#[derive(Clone, Debug, Serialize)]
pub struct ProposalRequest {
    /// The unique identifier of the proposal to retrieve
    pub proposal_id: Repr<ProposalId>,
}

/// Request to retrieve a paginated list of proposals.
///
/// # Fields
///
/// * `offset` - The number of proposals to skip from the beginning
/// * `length` - The maximum number of proposals to return
#[derive(Copy, Clone, Debug, Serialize, CandidType)]
pub struct ProposalsRequest {
    /// The number of proposals to skip from the beginning
    pub offset: usize,
    /// The maximum number of proposals to return
    pub length: usize,
}

// ============================================================================
// Mutate Request Types
// ============================================================================

/// Request to create a new proposal.
///
/// # Fields
///
/// * `proposal` - The proposal details including ID, author, and actions
#[derive(Clone, Debug, Serialize)]
pub struct ProposeRequest {
    /// The proposal details
    pub proposal: Proposal,
}

/// Request to approve an existing proposal.
///
/// # Fields
///
/// * `approval` - The approval details including proposal ID and signer
#[derive(Clone, Debug, Serialize)]
pub struct ApproveRequest {
    /// The approval details
    pub approval: ProposalApprovalWithSigner,
}

// ============================================================================
// Helper Functions for Creating Requests
// ============================================================================

impl ProposeRequest {
    /// Creates a new proposal request.
    ///
    /// # Arguments
    ///
    /// * `proposal_id` - A unique identifier for the proposal
    /// * `author_id` - The identity of the proposal author
    /// * `actions` - A vector of actions to be executed if the proposal is approved
    pub fn new(proposal_id: ProposalId, author_id: SignerId, actions: Vec<ProposalAction>) -> Self {
        Self {
            proposal: Proposal {
                id: Repr::new(proposal_id),
                author_id: Repr::new(author_id),
                actions,
            },
        }
    }
}

impl ApproveRequest {
    /// Creates a new approval request.
    ///
    /// # Arguments
    ///
    /// * `signer_id` - The identity of the signer approving the proposal
    /// * `proposal_id` - The unique identifier of the proposal to approve
    pub fn new(signer_id: SignerId, proposal_id: ProposalId) -> Self {
        Self {
            approval: ProposalApprovalWithSigner {
                proposal_id: Repr::new(proposal_id),
                signer_id: Repr::new(signer_id),
                added_timestamp: 0, // TODO: add timestamp
            },
        }
    }
}

// ============================================================================
// Re-exports for backward compatibility
// ============================================================================

// Re-export the ProxyMutateRequest enum for use in mutate.rs
// pub use crate::ProxyMutateRequest;  // Unused re-export
