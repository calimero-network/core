//! Context proxy mutation client for interacting with context management operations.
//!
//! This module provides a high-level interface for mutating context-related data
//! across different blockchain protocols. It abstracts away protocol-specific
//! encoding/decoding details and provides a unified API for:
//!
//! - Proposal creation and management
//! - Proposal approval workflows
//! - Signed transaction handling
//!
//! The client supports multiple protocols through protocol-specific implementations
//! in separate modules: `ethereum`, `icp`, `near`, `starknet`, and `stellar`.

use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::types::{ProposalId, SignerId};
use crate::{ProposalAction, ProposalWithApprovals, ProxyMutateRequest};

use super::requests::{ApproveRequest, ProposeRequest};

/// A client for mutating context-related data across different blockchain protocols.
///
/// This client provides methods to interact with context management operations
/// such as proposal creation and approval workflows. It automatically handles
/// protocol-specific encoding and decoding based on the configured transport.
///
/// # Example
///
/// ```rust,ignore
/// // Create a mutation client and use it to propose changes
/// let mutate_client = ContextProxyMutate { client: your_client };
/// let request = mutate_client.propose(proposal_id, author_id, actions);
/// let result = request.send(signing_key).await?;
/// ```
#[derive(Debug)]
pub struct ContextProxyMutate<'a, T> {
    /// The underlying call client for making requests
    pub client: CallClient<'a, T>,
}

/// A request builder for context proxy mutations.
///
/// This struct holds the mutation request data and provides a method to send
/// the request with the appropriate signing key for the target protocol.
///
/// # Example
///
/// ```rust,ignore
/// let request = mutate_client.approve(signer_id, proposal_id);
/// let result = request.send(signing_key).await?;
/// ```
#[derive(Debug)]
pub struct ContextProxyMutateRequest<'a, T> {
    client: CallClient<'a, T>,
    raw_request: ProxyMutateRequest,
}

/// Internal struct for handling mutation requests with signing.
///
/// This struct is used internally by the protocol-specific implementations
/// to handle the signing and encoding of mutation requests.
#[derive(Debug)]
pub struct Mutate {
    pub(crate) signing_key: [u8; 32],
    pub(crate) raw_request: ProxyMutateRequest,
}

impl<'a, T> ContextProxyMutate<'a, T> {
    /// Creates a new proposal mutation request.
    ///
    /// This method creates a request to propose a new set of actions to be executed
    /// in the context. The proposal will need to be approved by other signers before
    /// it can be executed.
    ///
    /// # Arguments
    ///
    /// * `proposal_id` - A unique identifier for the proposal
    /// * `author_id` - The identity of the proposal author
    /// * `actions` - A vector of actions to be executed if the proposal is approved
    ///
    /// # Returns
    ///
    /// Returns a `ContextProxyMutateRequest` that can be sent using the `send` method.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let request = mutate_client.propose(
    ///     proposal_id,
    ///     author_id,
    ///     vec![ProposalAction::SetVariable { key: b"config".to_vec(), value: b"new_value".to_vec() }]
    /// );
    /// let result = request.send(signing_key).await?;
    /// ```
    pub fn propose(
        self,
        proposal_id: ProposalId,
        author_id: SignerId,
        actions: Vec<ProposalAction>,
    ) -> ContextProxyMutateRequest<'a, T> {
        let propose_request = ProposeRequest::new(proposal_id, author_id, actions);
        ContextProxyMutateRequest {
            client: self.client,
            raw_request: ProxyMutateRequest::Propose {
                proposal: propose_request.proposal,
            },
        }
    }

    /// Creates a proposal approval mutation request.
    ///
    /// This method creates a request to approve an existing proposal. Once enough
    /// approvals are collected, the proposal can be executed.
    ///
    /// # Arguments
    ///
    /// * `signer_id` - The identity of the signer approving the proposal
    /// * `proposal_id` - The unique identifier of the proposal to approve
    ///
    /// # Returns
    ///
    /// Returns a `ContextProxyMutateRequest` that can be sent using the `send` method.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let request = mutate_client.approve(signer_id, proposal_id);
    /// let result = request.send(signing_key).await?;
    /// ```
    pub fn approve(
        self,
        signer_id: SignerId,
        proposal_id: ProposalId,
    ) -> ContextProxyMutateRequest<'a, T> {
        let approve_request = ApproveRequest::new(signer_id, proposal_id);
        ContextProxyMutateRequest {
            client: self.client,
            raw_request: ProxyMutateRequest::Approve {
                approval: approve_request.approval,
            },
        }
    }
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

impl<'a, T: Transport> ContextProxyMutateRequest<'a, T> {
    /// Sends the mutation request with the provided signing key.
    ///
    /// This method signs the request using the provided ED25519 private key and
    /// sends it to the context proxy. The signing process is protocol-specific
    /// and handled automatically based on the configured transport.
    ///
    /// # Arguments
    ///
    /// * `signing_key` - A 32-byte ED25519 private key for signing the request
    ///
    /// # Returns
    ///
    /// Returns the result of the mutation operation, which may include:
    /// - `Some(ProposalWithApprovals)` if the mutation was successful and created/updated a proposal
    /// - `None` if the mutation was successful but didn't result in a proposal
    /// - `ClientError<T>` if the request failed
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let signing_key = [0u8; 32]; // Your actual signing key
    /// let result = request.send(signing_key).await?;
    /// match result {
    ///     Some(proposal) => println!("Proposal created: {:?}", proposal),
    ///     None => println!("Mutation completed successfully"),
    /// }
    /// ```
    pub async fn send(
        self,
        signing_key: [u8; 32],
    ) -> Result<Option<ProposalWithApprovals>, ClientError<T>> {
        let request = Mutate {
            signing_key,
            raw_request: self.raw_request,
        };

        utils::send(&self.client, Operation::Write(request)).await
    }
}
