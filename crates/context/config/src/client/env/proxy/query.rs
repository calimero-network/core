use active_proposals::ActiveProposalRequest;
use context_storage_entries::ContextStorageEntriesRequest;
use context_variable::ContextVariableRequest;
use proposal::ProposalRequest;
use proposal_approvals::ProposalApprovalsRequest;
use proposal_approvers::ProposalApproversRequest;
use proposals::ProposalsRequest;

use crate::client::env::utils;
use crate::client::transport::Transport;
use crate::client::{CallClient, ClientError, Operation};
use crate::repr::Repr;
use crate::types::{ContextIdentity, ContextStorageEntry};
use crate::{Proposal, ProposalId, ProposalWithApprovals};

mod active_proposals;
mod context_storage_entries;
mod context_variable;
mod proposal;
mod proposal_approvals;
mod proposal_approvers;
mod proposals;

#[derive(Debug)]
pub struct ContextProxyQuery<'a, T> {
    pub client: CallClient<'a, T>,
}

impl<'a, T: Transport> ContextProxyQuery<'a, T> {
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

    pub async fn proposal(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Option<Proposal>, ClientError<T>> {
        let params = ProposalRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_number_of_active_proposals(&self) -> Result<u16, ClientError<T>> {
        let params = ActiveProposalRequest;

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_number_of_proposal_approvals(
        &self,
        proposal_id: ProposalId,
    ) -> Result<ProposalWithApprovals, ClientError<T>> {
        let params = ProposalApprovalsRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_proposal_approvers(
        &self,
        proposal_id: ProposalId,
    ) -> Result<Vec<ContextIdentity>, ClientError<T>> {
        let params = ProposalApproversRequest {
            proposal_id: Repr::new(proposal_id),
        };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_context_value(&self, key: Vec<u8>) -> Result<Vec<u8>, ClientError<T>> {
        let params = ContextVariableRequest { key };

        utils::send(&self.client, Operation::Read(params)).await
    }

    pub async fn get_context_storage_entries(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<ContextStorageEntry>, ClientError<T>> {
        let params = ContextStorageEntriesRequest { offset, limit };

        utils::send(&self.client, Operation::Read(params)).await
    }
}
