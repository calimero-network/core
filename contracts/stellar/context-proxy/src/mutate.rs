use calimero_context_config::stellar::stellar_types::{
    StellarSignedRequest, StellarSignedRequestPayload,
};
use calimero_context_config::stellar::{
    StellarProposal, StellarProposalAction, StellarProposalApprovalWithSigner,
    StellarProposalWithApprovals, StellarProxyError, StellarProxyMutateRequest,
};
use soroban_sdk::token::TokenClient;
use soroban_sdk::{
    contractimpl, log, vec, Address, BytesN, Env, FromVal, IntoVal, String, Symbol, TryFromVal,
    Val, Vec,
};

use crate::{ContextProxyContract, ContextProxyContractArgs, ContextProxyContractClient};

#[contractimpl]
impl ContextProxyContract {
    pub fn mutate(
        env: Env,
        signed_request: StellarSignedRequest,
    ) -> Result<Option<StellarProposalWithApprovals>, StellarProxyError> {
        log!(&env, "Mutating proposal");
        // Verify signature and get the payload in one step
        let verified_payload = signed_request
            .verify(&env)
            .map_err(|_| StellarProxyError::Unauthorized)?;

        // Process the verified payload
        match verified_payload {
            StellarSignedRequestPayload::Proxy(proxy_request) => match proxy_request {
                StellarProxyMutateRequest::Propose(proposal) => {
                    Self::internal_create_proposal(&env, proposal)
                }
                StellarProxyMutateRequest::Approve(approval) => {
                    Self::internal_approve_proposal(&env, approval)
                }
            },
            StellarSignedRequestPayload::Context(_) => Err(StellarProxyError::InvalidAction),
        }
    }

    fn internal_create_proposal(
        env: &Env,
        proposal: StellarProposal,
    ) -> Result<Option<StellarProposalWithApprovals>, StellarProxyError> {
        // Check membership
        if !Self::check_member(env, &proposal.author_id)? {
            return Err(StellarProxyError::Unauthorized);
        }

        if proposal.actions.is_empty() {
            return Err(StellarProxyError::InvalidAction);
        }

        // Check if the proposal contains a delete action
        for action in proposal.actions.iter() {
            if let StellarProposalAction::DeleteProposal(proposal_id) = action {
                // Get the proposal to be deleted
                let state = Self::get_state(env);
                let to_delete = state
                    .proposals
                    .get(proposal_id.clone())
                    .ok_or(StellarProxyError::ProposalNotFound)?;

                // Check if the current user is the author of the proposal to be deleted
                if to_delete.author_id != proposal.author_id {
                    return Err(StellarProxyError::Unauthorized);
                }

                Self::remove_proposal(env, &proposal_id);
                return Ok(None);
            }
        }

        let mut state = Self::get_state(env);

        // Check proposal limit
        let num_proposals = state
            .num_proposals_pk
            .get(proposal.author_id.clone())
            .unwrap_or(0);

        if num_proposals >= state.active_proposals_limit {
            return Err(StellarProxyError::TooManyActiveProposals);
        }

        // Validate proposal actions
        for action in proposal.actions.iter() {
            Self::validate_proposal_action(&action)?;
        }

        // Store proposal
        let proposal_id = proposal.id.clone();
        let author_id = proposal.author_id.clone();
        state.proposals.set(proposal_id.clone(), proposal);

        Self::save_state(env, &state);

        // Auto-approve by author
        Self::internal_approve_proposal(
            env,
            StellarProposalApprovalWithSigner {
                proposal_id,
                signer_id: author_id,
            },
        )
    }

    fn internal_approve_proposal(
        env: &Env,
        approval: StellarProposalApprovalWithSigner,
    ) -> Result<Option<StellarProposalWithApprovals>, StellarProxyError> {
        // Check membership
        if !Self::check_member(env, &approval.signer_id)? {
            return Err(StellarProxyError::Unauthorized);
        }

        let mut state = Self::get_state(env);
        let proposal_id = approval.proposal_id.clone();

        // Check if proposal exists
        if !state.proposals.contains_key(proposal_id.clone()) {
            return Err(StellarProxyError::ProposalNotFound);
        }

        // Get or create approvals vector
        let mut approvals = state
            .approvals
            .get(proposal_id.clone())
            .unwrap_or_else(|| Vec::new(env));

        // Check if already approved
        if approvals.contains(&approval.signer_id) {
            return Err(StellarProxyError::ProposalAlreadyApproved);
        }

        // Add approval
        approvals.push_back(approval.signer_id);
        state.approvals.set(proposal_id.clone(), approvals.clone());

        // Check if we need to execute
        let should_execute = approvals.len() >= state.num_approvals as u32;

        Self::save_state(env, &state);

        // Execute if needed
        if should_execute {
            Self::execute_proposal(env, &proposal_id)?;
            Ok(None)
        } else {
            Ok(Some(StellarProposalWithApprovals {
                proposal_id,
                num_approvals: approvals.len(),
            }))
        }
    }

    fn check_member(env: &Env, signer_id: &BytesN<32>) -> Result<bool, StellarProxyError> {
        // Get contract state to access context_config_id and context_id
        let state = ContextProxyContract::get_state(env);

        log!(&env, "Context config id: {:?}", state.context_config_id);
        log!(&env, "Context id: {:?}", state.context_id);
        // Make cross-contract call to check membership
        let has_member: bool = env.invoke_contract(
            &state.context_config_id,
            &Symbol::new(env, "has_member"),
            vec![env, state.context_id.into_val(env), signer_id.into_val(env)],
        );

        Ok(has_member)
    }

    fn validate_proposal_action(action: &StellarProposalAction) -> Result<(), StellarProxyError> {
        match action {
            StellarProposalAction::ExternalFunctionCall(_, method_name, _, deposit) => {
                if method_name.is_empty() || *deposit < 0 {
                    return Err(StellarProxyError::InvalidAction);
                }
            }
            StellarProposalAction::Transfer(_, amount) => {
                if *amount <= 0 {
                    return Err(StellarProxyError::InvalidAction);
                }
            }
            StellarProposalAction::SetNumApprovals(num_approvals) => {
                if *num_approvals == 0 {
                    return Err(StellarProxyError::InvalidAction);
                }
            }
            StellarProposalAction::SetActiveProposalsLimit(active_proposals_limit) => {
                if *active_proposals_limit == 0 {
                    return Err(StellarProxyError::InvalidAction);
                }
            }
            StellarProposalAction::SetContextValue(_, _) => {}
            StellarProposalAction::DeleteProposal(_) => {}
        }
        Ok(())
    }

    fn remove_proposal(env: &Env, proposal_id: &BytesN<32>) {
        let mut state = Self::get_state(env);

        // Get the proposal first to access author_id
        if let Some(proposal) = state.proposals.get(proposal_id.clone()) {
            // Remove approvals
            state.approvals.remove(proposal_id.clone());

            // Remove proposal
            state.proposals.remove(proposal_id.clone());

            // Update author count
            if let Some(count) = state.num_proposals_pk.get(proposal.author_id.clone()) {
                if count <= 1 {
                    state.num_proposals_pk.remove(proposal.author_id.clone());
                } else {
                    state
                        .num_proposals_pk
                        .set(proposal.author_id.clone(), count - 1);
                }
            }

            Self::save_state(env, &state);
        }
    }

    fn execute_proposal(env: &Env, proposal_id: &BytesN<32>) -> Result<(), StellarProxyError> {
        let state = Self::get_state(env);
        let proposal = state
            .proposals
            .get(proposal_id.clone())
            .ok_or(StellarProxyError::ProposalNotFound)?;

        // Execute each action
        for action in proposal.actions.iter() {
            match action {
                StellarProposalAction::ExternalFunctionCall(
                    receiver_id,
                    method_name,
                    args,
                    deposit,
                ) => {
                    // If there's a deposit, handle the XLM transfer first
                    if deposit > 0 {
                        let token_client = TokenClient::new(env, &state.ledger_id);
                        let contract_address = env.current_contract_address();

                        // Check balance and transfer
                        let balance = token_client.balance(&contract_address);
                        if balance < deposit {
                            return Err(StellarProxyError::InsufficientBalance);
                        }

                        token_client.transfer(&contract_address, &receiver_id, &deposit);
                    }

                    // Convert the String to Vec<Val>
                    let mut vec_val: Vec<Val> = Vec::new(env);
                    let arg_val: Val = args.into_val(env);
                    vec_val.push_back(arg_val);

                    // Make the cross-contract call
                    env.invoke_contract::<()>(
                        &receiver_id,
                        &Symbol::from_val(env, &method_name.to_val()),
                        vec_val,
                    );
                }

                StellarProposalAction::Transfer(receiver_id, amount) => {
                    log!(&env, "Transferring {} to {}", amount, receiver_id);
                    let token_client = TokenClient::new(env, &state.ledger_id);
                    let contract_address = env.current_contract_address();

                    token_client.transfer(&contract_address, &receiver_id, &amount);
                }

                StellarProposalAction::SetNumApprovals(num_approvals) => {
                    let mut state = Self::get_state(env);
                    state.num_approvals = num_approvals;
                    Self::save_state(env, &state);
                }

                StellarProposalAction::SetActiveProposalsLimit(active_proposals_limit) => {
                    let mut state = Self::get_state(env);
                    state.active_proposals_limit = active_proposals_limit;
                    Self::save_state(env, &state);
                }

                StellarProposalAction::SetContextValue(key, value) => {
                    let mut state = Self::get_state(env);
                    state.context_storage.set(key.clone(), value.clone());
                    Self::save_state(env, &state);
                }

                StellarProposalAction::DeleteProposal(proposal_id) => {
                    Self::remove_proposal(env, &proposal_id);
                }
            }
        }

        // Clean up after successful execution
        Self::remove_proposal(env, proposal_id);
        Ok(())
    }
}
