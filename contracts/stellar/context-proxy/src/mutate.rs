use calimero_context_config::stellar::stellar_types::{
    StellarSignedRequest, StellarSignedRequestPayload,
};
use calimero_context_config::stellar::{
    StellarProposal, StellarProposalAction, StellarProposalApprovalWithSigner,
    StellarProposalWithApprovals, StellarProxyError, StellarProxyMutateRequest,
};
use soroban_sdk::auth::{ContractContext, InvokerContractAuthEntry, SubContractInvocation};
use soroban_sdk::token::TokenClient;
use soroban_sdk::{contractimpl, log, symbol_short, vec, BytesN, Env, IntoVal, Symbol, Val, Vec};

use crate::{ContextProxyContract, ContextProxyContractArgs, ContextProxyContractClient};

#[contractimpl]
impl ContextProxyContract {
    /// Processes a signed mutation request for the proxy contract
    /// # Arguments
    /// * `signed_request` - The signed request containing the mutation action
    /// # Errors
    /// * Returns Unauthorized if signature verification fails
    /// * Returns InvalidAction for invalid request payload
    pub fn mutate(
        env: Env,
        signed_request: StellarSignedRequest,
    ) -> Result<Option<StellarProposalWithApprovals>, StellarProxyError> {
        // Verify signature and get the payload
        let verified_payload = signed_request
            .verify(&env)
            .map_err(|_| StellarProxyError::Unauthorized)?;

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

    /// Creates a new proposal in the contract
    /// # Arguments
    /// * `proposal` - The proposal to be created
    /// # Errors
    /// * Returns Unauthorized if author is not a member
    /// * Returns InvalidAction if proposal has no actions
    /// * Returns TooManyActiveProposals if author exceeds proposal limit
    fn internal_create_proposal(
        env: &Env,
        proposal: StellarProposal,
    ) -> Result<Option<StellarProposalWithApprovals>, StellarProxyError> {
        // Check membership and validate proposal
        if !Self::is_member(env, &proposal.author_id)? {
            return Err(StellarProxyError::Unauthorized);
        }

        if proposal.actions.is_empty() {
            return Err(StellarProxyError::InvalidAction);
        }

        // Handle delete action if present
        if let Some(delete_action) = proposal
            .actions
            .iter()
            .find(|action| matches!(action, StellarProposalAction::DeleteProposal(_)))
        {
            if let StellarProposalAction::DeleteProposal(proposal_id) = delete_action {
                let state = Self::get_state(env);
                let to_delete = state
                    .proposals
                    .get(proposal_id.clone())
                    .ok_or(StellarProxyError::ProposalNotFound)?;

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

        // Validate all actions
        proposal
            .actions
            .iter()
            .try_for_each(|action| Self::validate_proposal_action(&action))?;

        // Store proposal
        let proposal_id = proposal.id.clone();
        let author_id = proposal.author_id.clone();
        state.proposals.set(proposal_id.clone(), proposal);

        // Increment the number of proposals for this author
        state
            .num_proposals_pk
            .set(author_id.clone(), num_proposals + 1);

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

    /// Approves an existing proposal
    /// # Arguments
    /// * `approval` - The approval details including proposal ID and signer
    /// # Errors
    /// * Returns Unauthorized if signer is not a member
    /// * Returns ProposalNotFound if proposal doesn't exist
    /// * Returns ProposalAlreadyApproved if signer already approved
    fn internal_approve_proposal(
        env: &Env,
        approval: StellarProposalApprovalWithSigner,
    ) -> Result<Option<StellarProposalWithApprovals>, StellarProxyError> {
        if !Self::is_member(env, &approval.signer_id)? {
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

        // Add approval and update state
        approvals.push_back(approval.signer_id);
        state.approvals.set(proposal_id.clone(), approvals.clone());

        let should_execute = approvals.len() >= state.num_approvals as u32;
        Self::save_state(env, &state);

        if should_execute {
            Self::execute_proposal(env, &proposal_id)?;
            return Ok(None);
        }

        Ok(Some(StellarProposalWithApprovals {
            proposal_id,
            num_approvals: approvals.len(),
        }))
    }

    /// Verifies if an address is a member of the context
    /// # Errors
    /// Returns error from context contract if verification fails
    fn is_member(env: &Env, signer_id: &BytesN<32>) -> Result<bool, StellarProxyError> {
        let state = Self::get_state(env);

        let args = vec![env, state.context_id.into_val(env), signer_id.into_val(env)];

        let has_member: bool = env.invoke_contract(
            &state.context_config_id,
            &Symbol::new(env, "has_member"),
            args,
        );

        Ok(has_member)
    }

    /// Validates a single proposal action
    /// # Errors
    /// Returns InvalidAction if the action parameters are invalid
    fn validate_proposal_action(action: &StellarProposalAction) -> Result<(), StellarProxyError> {
        match action {
            StellarProposalAction::ExternalFunctionCall(_, method_name, _, deposit)
                if method_name.to_val().is_void() || *deposit < 0 =>
            {
                Err(StellarProxyError::InvalidAction)
            }
            StellarProposalAction::Transfer(_, amount) if *amount <= 0 => {
                Err(StellarProxyError::InvalidAction)
            }
            StellarProposalAction::SetNumApprovals(num_approvals) if *num_approvals == 0 => {
                Err(StellarProxyError::InvalidAction)
            }
            StellarProposalAction::SetActiveProposalsLimit(limit) if *limit == 0 => {
                Err(StellarProxyError::InvalidAction)
            }
            _ => Ok(()),
        }
    }

    /// Removes a proposal and updates related state
    fn remove_proposal(env: &Env, proposal_id: &BytesN<32>) {
        let mut state = Self::get_state(env);

        if let Some(proposal) = state.proposals.get(proposal_id.clone()) {
            let author_id = proposal.author_id.clone();

            // Batch removals
            state.approvals.remove(proposal_id.clone());
            state.proposals.remove(proposal_id.clone());

            // Update author count
            if let Some(count) = state.num_proposals_pk.get(author_id.clone()) {
                if count <= 1 {
                    state.num_proposals_pk.remove(author_id);
                } else {
                    state.num_proposals_pk.set(author_id, count - 1);
                }
            }

            Self::save_state(env, &state);
        }
    }

    /// Executes a proposal that has received sufficient approvals
    /// # Errors
    /// * Returns ProposalNotFound if proposal doesn't exist
    /// * Returns InsufficientBalance for failed token transfers
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
                    // Handle deposit if present
                    if deposit > 0 {
                        let token_client = TokenClient::new(env, &state.ledger_id);
                        let current_ledger = env.ledger().sequence();
                        let expiration_ledger = current_ledger + 100;
                        let contract_address = env.current_contract_address();

                        // Verify balance and approve transfer
                        let balance = token_client.balance(&contract_address);
                        if balance < deposit {
                            return Err(StellarProxyError::InsufficientBalance);
                        }

                        // Auth for token approve
                        let token_auth =
                            InvokerContractAuthEntry::Contract(SubContractInvocation {
                                context: ContractContext {
                                    contract: state.ledger_id.clone(),
                                    fn_name: symbol_short!("approve"),
                                    args: (
                                        &contract_address,
                                        &receiver_id,
                                        &deposit,
                                        &expiration_ledger,
                                    )
                                        .into_val(env),
                                },
                                sub_invocations: vec![&env],
                            });

                        // Authorize token operation separately
                        env.authorize_as_current_contract(vec![&env, token_auth]);

                        // Execute token approve
                        token_client.approve(
                            &contract_address,
                            &receiver_id,
                            &deposit,
                            &expiration_ledger,
                        );
                    }

                    let contract_address = env.current_contract_address();

                    env.authorize_as_current_contract(vec![
                        &env,
                        InvokerContractAuthEntry::Contract(SubContractInvocation {
                            context: ContractContext {
                                contract: receiver_id.clone(),
                                fn_name: symbol_short!("transfer"),
                                args: (contract_address.clone(), receiver_id.clone(), deposit)
                                    .into_val(env),
                            },
                            sub_invocations: vec![&env],
                        }),
                    ]);

                    // Execute external call
                    env.invoke_contract::<Val>(&receiver_id, &method_name, args);

                    // Handle post-call deposit if needed
                    if deposit > 0 {
                        let token_client = TokenClient::new(env, &state.ledger_id);
                        let current_ledger = env.ledger().sequence();
                        let expiration_ledger = current_ledger + 100;
                        let contract_address = env.current_contract_address();

                        // Auth for token approve
                        let token_auth =
                            InvokerContractAuthEntry::Contract(SubContractInvocation {
                                context: ContractContext {
                                    contract: state.ledger_id.clone(),
                                    fn_name: symbol_short!("approve"),
                                    args: (
                                        &contract_address,
                                        &receiver_id,
                                        &deposit,
                                        &expiration_ledger,
                                    )
                                        .into_val(env),
                                },
                                sub_invocations: vec![&env],
                            });

                        // Authorize token operation separately
                        env.authorize_as_current_contract(vec![&env, token_auth]);

                        // Execute token approve
                        token_client.approve(
                            &contract_address,
                            &receiver_id,
                            &0,
                            &expiration_ledger,
                        );
                    }
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

                StellarProposalAction::SetActiveProposalsLimit(limit) => {
                    let mut state = Self::get_state(env);
                    state.active_proposals_limit = limit;
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
