use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::{
    ICProposal, ICProposalApprovalWithSigner, ICProposalWithApprovals,
};
use calimero_context_config::types::{ProposalId, SignerId};

use crate::with_state;

#[ic_cdk::query]
pub fn get_num_approvals() -> u32 {
    with_state(|contract| contract.num_approvals)
}

#[ic_cdk::query]
pub fn get_active_proposals_limit() -> u32 {
    with_state(|contract| contract.active_proposals_limit)
}

#[ic_cdk::query]
pub fn proposal(proposal_id: ICRepr<ProposalId>) -> Option<ICProposal> {
    with_state(|contract| contract.proposals.get(&proposal_id).cloned())
}

#[ic_cdk::query]
pub fn proposals(from_index: usize, limit: usize) -> Vec<ICProposal> {
    with_state(|contract| {
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
pub fn get_confirmations_count(proposal_id: ICRepr<ProposalId>) -> Option<ICProposalWithApprovals> {
    with_state(|contract| {
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
pub fn get_proposal_approvers(proposal_id: ICRepr<ProposalId>) -> Option<Vec<ICRepr<SignerId>>> {
    with_state(|contract| {
        contract
            .approvals
            .get(&proposal_id)
            .map(|approvals| approvals.iter().cloned().collect())
    })
}

#[ic_cdk::query]
pub fn get_proposal_approvals_with_signer(
    proposal_id: ICRepr<ProposalId>,
) -> Vec<ICProposalApprovalWithSigner> {
    with_state(|contract| {
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
    with_state(|contract| contract.context_storage.get(&key).cloned())
}

#[ic_cdk::query]
pub fn context_storage_entries(from_index: usize, limit: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    with_state(|contract| {
        contract
            .context_storage
            .iter()
            .skip(from_index)
            .take(limit)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    })
}
