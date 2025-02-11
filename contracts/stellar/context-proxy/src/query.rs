use calimero_context_config::stellar::{
    StellarProposal, StellarProposalApprovalWithSigner, StellarProposalWithApprovals,
};
use soroban_sdk::{contractimpl, Bytes, BytesN, Env, Vec};

use crate::{ContextProxyContract, ContextProxyContractArgs, ContextProxyContractClient};

#[contractimpl]
impl ContextProxyContract {
    /// Returns the number of approvals required for proposal execution
    pub fn get_num_approvals(env: Env) -> u32 {
        Self::get_state(&env).num_approvals
    }

    /// Returns the maximum number of active proposals allowed per author
    pub fn get_active_proposals_limit(env: Env) -> u32 {
        Self::get_state(&env).active_proposals_limit
    }

    /// Retrieves a specific proposal by ID
    /// # Arguments
    /// * `proposal_id` - The ID of the proposal to retrieve
    pub fn proposal(env: Env, proposal_id: BytesN<32>) -> Option<StellarProposal> {
        Self::get_state(&env).proposals.get(proposal_id)
    }

    /// Returns a paginated list of active proposals
    /// # Arguments
    /// * `from_index` - Starting index for pagination
    /// * `limit` - Maximum number of proposals to return
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

    /// Gets the number of confirmations for a specific proposal
    /// # Arguments
    /// * `proposal_id` - The ID of the proposal to check
    /// # Returns
    /// Returns None if the proposal doesn't exist, otherwise returns the proposal ID and number of approvals
    pub fn get_confirmations_count(
        env: Env,
        proposal_id: BytesN<32>,
    ) -> Option<StellarProposalWithApprovals> {
        let state = Self::get_state(&env);

        state.proposals.get(proposal_id.clone()).map(|_| {
            let num_approvals = state
                .approvals
                .get(proposal_id.clone())
                .map_or(0, |approvals| approvals.len());

            StellarProposalWithApprovals {
                proposal_id,
                num_approvals,
            }
        })
    }

    /// Returns the list of addresses that have approved a specific proposal
    /// # Arguments
    /// * `proposal_id` - The ID of the proposal to check
    pub fn proposal_approvers(env: Env, proposal_id: BytesN<32>) -> Option<Vec<BytesN<32>>> {
        Self::get_state(&env)
            .approvals
            .get(proposal_id)
            .map(|approvals| approvals.clone())
    }

    /// Returns detailed approval information for a proposal
    /// # Arguments
    /// * `proposal_id` - The ID of the proposal to check
    /// # Returns
    /// Returns a vector of approval records containing both proposal ID and signer ID
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

    /// Retrieves a value from the context storage
    /// # Arguments
    /// * `key` - The key to look up in the context storage
    pub fn get_context_value(env: Env, key: Bytes) -> Option<Bytes> {
        Self::get_state(&env).context_storage.get(key)
    }

    /// Returns a paginated list of key-value pairs from the context storage
    /// # Arguments
    /// * `from_index` - Starting index for pagination
    /// * `limit` - Maximum number of entries to return
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
