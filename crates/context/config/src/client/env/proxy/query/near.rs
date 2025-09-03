#![cfg(feature = "near_client")]

//! NEAR Protocol specific implementations for context proxy queries.
//!
//! This module provides NEAR Protocol blockchain-specific implementations of the
//! `Method<Near>` trait for all context proxy query operations. It handles NEAR's
//! JSON-based serialization format using the `serde_json` crate.
//!
//! ## Key Features
//!
//! - **JSON Serialization**: Uses JSON format for parameter encoding and response decoding
//! - **Simple Integration**: Leverages standard JSON for easy debugging and inspection
//! - **NEAR Compatibility**: Optimized for NEAR's view function calls and RPC interface
//! - **Error Handling**: Converts NEAR-specific errors to generic `eyre::Result`
//!
//! ## Implementation Details
//!
//! Each request type is encoded using JSON serialization:
//! - All request structs implement `Serialize` for JSON encoding
//! - Responses are decoded using `serde_json::from_slice` for type safety
//! - NEAR-specific types are handled through the `Repr` wrapper system
//! - Simple and efficient for NEAR's view function architecture
//!
//! ## Usage
//!
//! These implementations are used automatically by the `ContextProxyQuery` client
//! when the underlying transport is configured for NEAR Protocol. No direct usage is required.

use serde::Serialize;

use super::super::requests::{
    ActiveProposalRequest, ContextStorageEntriesRequest, ContextVariableRequest,
    ProposalApprovalsRequest, ProposalApproversRequest, ProposalRequest, ProposalsRequest,
};
use crate::client::env::Method;
use crate::client::protocol::near::Near;
use crate::repr::Repr;
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalId, ProposalWithApprovals};

impl Method<Near> for ActiveProposalRequest {
    const METHOD: &'static str = "get_active_proposals_limit";
    type Returns = u16;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Near> for ContextStorageEntriesRequest {
    const METHOD: &'static str = "context_storage_entries";
    type Returns = Vec<ContextStorageEntry>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let entries: Vec<(Box<[u8]>, Box<[u8]>)> = serde_json::from_slice(&response)?;
        Ok(entries
            .into_iter()
            .map(|(key, value)| ContextStorageEntry {
                key: key.into(),
                value: value.into(),
            })
            .collect())
    }
}

impl Method<Near> for ContextVariableRequest {
    const METHOD: &'static str = "get_context_value";
    type Returns = Vec<u8>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Near> for ProposalApprovalsRequest {
    const METHOD: &'static str = "get_confirmations_count";
    type Returns = ProposalWithApprovals;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Near> for ProposalApproversRequest {
    const METHOD: &'static str = "get_proposal_approvers";
    type Returns = Vec<ContextIdentity>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        let members: Vec<Repr<ContextIdentity>> = serde_json::from_slice(&response)?;
        #[expect(
            clippy::transmute_undefined_repr,
            reason = "Repr<T> is a transparent wrapper around T"
        )]
        let members = unsafe {
            std::mem::transmute::<Vec<Repr<ContextIdentity>>, Vec<ContextIdentity>>(members)
        };
        Ok(members)
    }
}

impl Method<Near> for ProposalRequest {
    const METHOD: &'static str = "proposal";
    type Returns = Option<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}

impl Method<Near> for ProposalsRequest {
    const METHOD: &'static str = "proposals";
    type Returns = Vec<Proposal>;
    fn encode(self) -> eyre::Result<Vec<u8>> {
        serde_json::to_vec(&self).map_err(Into::into)
    }
    fn decode(response: Vec<u8>) -> eyre::Result<Self::Returns> {
        serde_json::from_slice(&response).map_err(Into::into)
    }
}
