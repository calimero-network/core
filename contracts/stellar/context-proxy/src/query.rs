use calimero_context_config::stellar::{
    StellarProposal, StellarProposalApprovalWithSigner, StellarProposalWithApprovals,
};
use soroban_sdk::{contractimpl, Bytes, BytesN, Env, Vec};

use crate::{ContextProxyContract, ContextProxyContractArgs, ContextProxyContractClient};

#[contractimpl]
impl ContextProxyContract {
    #[allow(dead_code)]
    pub fn get_num_approvals(env: Env) -> u32 {
        Self::get_state(&env).num_approvals
    }

    #[allow(dead_code)]
    pub fn get_active_proposals_limit(env: Env) -> u32 {
        Self::get_state(&env).active_proposals_limit
    }

    #[allow(dead_code)]
    pub fn proposal(env: Env, proposal_id: BytesN<32>) -> Option<StellarProposal> {
        Self::get_state(&env).proposals.get(proposal_id)
    }

    #[allow(dead_code)]
    pub fn proposals(env: Env, from_index: u32, limit: u32) -> Vec<StellarProposal> {
        let state = Self::get_state(&env);
        let mut result = Vec::new(&env);

        for (_, proposal) in state
            .proposals
            .iter()
            .skip(from_index as usize)
            .take(limit as usize)
        {
            result.push_back(proposal);
        }
        result
    }

    #[allow(dead_code)]
    pub fn get_confirmations_count(
        env: Env,
        proposal_id: BytesN<32>,
    ) -> Option<StellarProposalWithApprovals> {
        let state = Self::get_state(&env);

        if state.proposals.contains_key(proposal_id.clone()) {
            let num_approvals = state
                .approvals
                .get(proposal_id.clone())
                .map_or(0, |approvals| approvals.len());

            Some(StellarProposalWithApprovals {
                proposal_id,
                num_approvals,
            })
        } else {
            None
        }
    }

    #[allow(dead_code)]
    pub fn proposal_approvers(env: Env, proposal_id: BytesN<32>) -> Option<Vec<BytesN<32>>> {
        Self::get_state(&env)
            .approvals
            .get(proposal_id)
            .map(|approvals| approvals.clone())
    }

    #[allow(dead_code)]
    pub fn proposal_approvals_with_signer(
        env: Env,
        proposal_id: BytesN<32>,
    ) -> Vec<StellarProposalApprovalWithSigner> {
        let state = Self::get_state(&env);
        let mut result = Vec::new(&env);

        if let Some(approvals) = state.approvals.get(proposal_id.clone()) {
            for signer_id in approvals.iter() {
                result.push_back(StellarProposalApprovalWithSigner {
                    proposal_id: proposal_id.clone(),
                    signer_id: signer_id.clone(),
                });
            }
        }
        result
    }

    #[allow(dead_code)]
    pub fn get_context_value(env: Env, key: Bytes) -> Option<Bytes> {
        Self::get_state(&env).context_storage.get(key)
    }

    #[allow(dead_code)]
    pub fn context_storage_entries(env: Env, from_index: u32, limit: u32) -> Vec<(Bytes, Bytes)> {
        let state = Self::get_state(&env);
        let mut result = Vec::new(&env);

        for (key, value) in state
            .context_storage
            .iter()
            .skip(from_index as usize)
            .take(limit as usize)
        {
            result.push_back((key.clone(), value.clone()));
        }
        result
    }
}
