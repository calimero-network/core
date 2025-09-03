//! Context proxy query client for interacting with context management operations.
//!
//! This module provides a high-level interface for querying context-related data
//! across different blockchain protocols. It abstracts away protocol-specific
//! encoding/decoding details and provides a unified API for:
//!
//! - Proposal management (listing, retrieving, approvals)
//! - Context storage operations (key-value storage)
//! - Context variable access
//!
//! The client supports multiple protocols through protocol-specific implementations
//! in separate modules: `ethereum`, `icp`, `near`, `starknet`, and `stellar`.

use candid::CandidType;
use serde::{Deserialize, Serialize};

use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::Repr;
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalId, ProposalWithApprovals};

/// A client for querying context-related data across different blockchain protocols.
///
/// This client provides methods to interact with context management operations
/// such as proposal queries, context storage access, and variable retrieval.
/// It automatically handles protocol-specific encoding and decoding based on
/// the configured transport.
///
/// # Example
///
/// ```rust,ignore
/// // Create a query client and use it to fetch proposals
/// let query_client = ContextProxyQuery { client: your_client };
/// let proposals = query_client.proposals(0, 10).await?;
/// let proposal = query_client.proposal(proposal_id).await?;
/// ```
#[derive(Debug)]
pub struct ContextProxyQuery<'a, T> {
    /// The underlying call client for making requests
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextProxyQuery<'a, T> {
    /// Retrieves a paginated list of proposals from the context.
    ///
    /// # Arguments
    ///
    /// * `offset` - The number of proposals to skip from the beginning
    /// * `limit` - The maximum number of proposals to return
    ///
    /// # Returns
    ///
    /// Returns a vector of proposals, or an error if the request fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// // Get the first 10 proposals
    /// let proposals = query_client.proposals(0, 10).await?;
    /// ```
    pub async fn proposals(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<Proposal>, ClientError<T>> {
        let params = ProposalsRequest {
            offset,
            length: limit,
        };
        utils::send(&self.client, Operation::Read(params)).await
    }

    /// Retrieves a specific proposal by its ID.
    ///
    /// # Arguments
    ///
    /// * `proposal_id` - The unique identifier of the proposal to retrieve
    ///
    /// # Returns
    ///
    /// Returns `Some(Proposal)` if the proposal exists, `None` if it doesn't,
    /// or an error if the request fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let proposal = query_client.proposal(proposal_id).await?;
    /// match proposal {
    ///     Some(p) => println!("Found proposal: {:?}", p),
    ///     None => println!("Proposal not found"),
    /// }
    /// ```
    pub async fn proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Option<Proposal>, ClientError<T>> {
        let params = ProposalRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    /// Gets the total number of active proposals in the context.
    ///
    /// # Returns
    ///
    /// Returns the count of active proposals, or an error if the request fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let count = query_client.get_number_of_active_proposals().await?;
    /// println!("There are {} active proposals", count);
    /// ```
    pub async fn get_number_of_active_proposals(&self) -> Result<u16, ClientError<T>> {
        let params = ActiveProposalRequest;

        utils::send(&self.client, Operation::Read(params)).await
    }

    /// Retrieves approval information for a specific proposal.
    ///
    /// # Arguments
    ///
    /// * `proposal_id` - The unique identifier of the proposal
    ///
    /// # Returns
    ///
    /// Returns a `ProposalWithApprovals` containing the proposal details
    /// and its approval status, or an error if the request fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let approvals = query_client.get_number_of_proposal_approvals(proposal_id).await?;
    /// println!("Proposal has {} approvals", approvals.approvals_count);
    /// ```
    pub async fn get_number_of_proposal_approvals(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalWithApprovals, ClientError<T>> {
        let params = ProposalApprovalsRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    /// Gets the list of identities that have approved a specific proposal.
    ///
    /// # Arguments
    ///
    /// * `proposal_id` - The unique identifier of the proposal
    ///
    /// # Returns
    ///
    /// Returns a vector of context identities that have approved the proposal,
    /// or an error if the request fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let approvers = query_client.get_proposal_approvers(proposal_id).await?;
    /// println!("Proposal approved by {} identities", approvers.len());
    /// ```
    pub async fn get_proposal_approvers(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Vec<ContextIdentity>, ClientError<T>> {
        let params = ProposalApproversRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    /// Retrieves a context variable value by its key.
    ///
    /// # Arguments
    ///
    /// * `key` - The byte array key identifying the context variable
    ///
    /// # Returns
    ///
    /// Returns the value associated with the key as a byte array,
    /// or an error if the request fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let key = b"my_context_variable".to_vec();
    /// let value = query_client.get_context_value(key).await?;
    /// println!("Context value: {:?}", value);
    /// ```
    pub async fn get_context_value(&self, key: Vec<u8>) -> Result<Vec<u8>, ClientError<T>> {
        let params = ContextVariableRequest { key };

        utils::send(&self.client, Operation::Read(params)).await
    }

    /// Retrieves a paginated list of context storage entries.
    ///
    /// # Arguments
    ///
    /// * `offset` - The number of entries to skip from the beginning
    /// * `limit` - The maximum number of entries to return
    ///
    /// # Returns
    ///
    /// Returns a vector of context storage entries (key-value pairs),
    /// or an error if the request fails.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let entries = query_client.get_context_storage_entries(0, 50).await?;
    /// for entry in entries {
    ///     println!("Key: {:?}, Value: {:?}", entry.key, entry.value);
    /// }
    /// ```
    pub async fn get_context_storage_entries(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ContextStorageEntry>, ClientError<T>> {
        let params = ContextStorageEntriesRequest { offset, limit };

        utils::send(&self.client, Operation::Read(params)).await
    }
}

// Request structs for context proxy queries

/// Request to get the number of active proposals in the context.
///
/// This is a simple request that doesn't require any parameters.
#[derive(Copy, Clone, Debug, Serialize, CandidType)]
pub(super) struct ActiveProposalRequest;

/// Request to retrieve paginated context storage entries.
///
/// # Fields
///
/// * `offset` - The number of entries to skip from the beginning
/// * `limit` - The maximum number of entries to return
#[derive(Clone, Debug, Serialize, CandidType)]
pub(super) struct ContextStorageEntriesRequest {
    /// The number of entries to skip from the beginning
    pub(super) offset: usize,
    /// The maximum number of entries to return
    pub(super) limit: usize,
}

/// Request to retrieve a context variable value by its key.
///
/// # Fields
///
/// * `key` - The byte array key identifying the context variable
#[derive(Clone, Debug, Serialize, CandidType)]
pub(super) struct ContextVariableRequest {
    /// The byte array key identifying the context variable
    pub(super) key: Vec<u8>,
}

/// Request to get approval information for a specific proposal.
///
/// # Fields
///
/// * `proposal_id` - The unique identifier of the proposal
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct ProposalApprovalsRequest {
    /// The unique identifier of the proposal
    pub(super) proposal_id: Repr<ProposalId>,
}

/// Request to get the list of identities that have approved a proposal.
///
/// # Fields
///
/// * `proposal_id` - The unique identifier of the proposal
#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalApproversRequest {
    /// The unique identifier of the proposal
    pub(super) proposal_id: Repr<ProposalId>,
}

/// Request to retrieve a specific proposal by its ID.
///
/// # Fields
///
/// * `proposal_id` - The unique identifier of the proposal to retrieve
#[derive(Clone, Debug, Serialize)]
pub(super) struct ProposalRequest {
    /// The unique identifier of the proposal to retrieve
    pub(super) proposal_id: Repr<ProposalId>,
}

/// Request to retrieve a paginated list of proposals.
///
/// # Fields
///
/// * `offset` - The number of proposals to skip from the beginning
/// * `length` - The maximum number of proposals to return
#[derive(Copy, Clone, Debug, Serialize, CandidType)]
pub(super) struct ProposalsRequest {
    /// The number of proposals to skip from the beginning
    pub(super) offset: usize,
    /// The maximum number of proposals to return
    pub(super) length: usize,
}

// Protocol-specific implementations
// These modules contain the actual Method trait implementations for each blockchain protocol
#[cfg(feature = "ethereum_client")]
mod ethereum;
#[cfg(feature = "icp_client")]
mod icp;
#[cfg(feature = "near_client")]
mod near;
#[cfg(feature = "starknet_client")]
mod starknet;
#[cfg(feature = "stellar_client")]
mod stellar;
