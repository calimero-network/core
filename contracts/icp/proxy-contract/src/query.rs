use calimero_context_config::repr::ReprTransmute;
use candid::Principal;

use crate::types::*;
use crate::PROXY_CONTRACT;

#[ic_cdk::query]
pub fn get_num_approvals() -> u32 {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract.num_approvals
    })
}

#[ic_cdk::query]
pub fn get_active_proposals_limit() -> u32 {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract.active_proposals_limit
    })
}

#[ic_cdk::query]
pub fn proposal(proposal_id: ICProposalId) -> Option<ICProposal> {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract.proposals.get(&proposal_id).cloned()
    })
}

#[ic_cdk::query]
pub fn proposals(from_index: usize, limit: usize) -> Vec<ICProposal> {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract
            .proposals
            .values()
            .skip(from_index)
            .take(limit)
            .cloned()
            .collect()
    })
}

#[ic_cdk::query]
pub fn get_confirmations_count(proposal_id: ICProposalId) -> Option<ICProposalWithApprovals> {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract.proposals.get(&proposal_id).map(|_| {
            let num_approvals = contract
                .approvals
                .get(&proposal_id)
                .map_or(0, |approvals| approvals.len());
            ICProposalWithApprovals {
                proposal_id,
                num_approvals,
            }
        })
    })
}

#[ic_cdk::query]
pub fn get_proposal_approvers(proposal_id: ICProposalId) -> Option<Vec<ICSignerId>> {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        if let Some(approvals) = contract.approvals.get(&proposal_id) {
            Some(approvals.iter().flat_map(|a| a.rt()).collect())
        } else {
            None
        }
    })
}

#[ic_cdk::query]
pub fn get_proposal_approvals_with_signer(
    proposal_id: ICProposalId,
) -> Vec<ICProposalApprovalWithSigner> {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        if let Some(approvals) = contract.approvals.get(&proposal_id) {
            approvals
                .iter()
                .map(|signer_id| ICProposalApprovalWithSigner {
                    proposal_id: proposal_id.clone(),
                    signer_id: signer_id.clone(),
                    added_timestamp: 0, // TODO: We need to store approval timestamps
                })
                .collect()
        } else {
            vec![]
        }
    })
}

#[ic_cdk::query]
pub fn get_context_value(key: Vec<u8>) -> Option<Vec<u8>> {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract.context_storage.get(&key).cloned()
    })
}

#[ic_cdk::query]
pub fn context_storage_entries(from_index: usize, limit: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract
            .context_storage
            .iter()
            .skip(from_index)
            .take(limit)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    })
}
