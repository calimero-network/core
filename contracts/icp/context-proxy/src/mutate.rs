use std::collections::BTreeSet;

use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::types::ICSigned;
use calimero_context_config::icp::{
    ICProposal, ICProposalAction, ICProposalApprovalWithSigner, ICProposalWithApprovals,
    ICProxyMutateRequest,
};
use calimero_context_config::types::{ProposalId, SignerId};
use candid::Principal;
use ic_cdk::api::call::CallResult;
use ic_ledger_types::{AccountIdentifier, Memo, Subaccount, Tokens, TransferArgs, TransferError};

use crate::{with_state, with_state_mut, ICProxyContract};

async fn check_member(signer_id: ICRepr<SignerId>) -> Result<bool, String> {
    let (context_canister_id, context_id) =
        with_state(|contract| (contract.context_config_id, contract.context_id));

    let call_result: CallResult<(bool,)> =
        ic_cdk::call(context_canister_id, "has_member", (context_id, signer_id)).await;

    match call_result {
        Ok((is_member,)) => Ok(is_member),
        Err(e) => Err(format!("Error checking membership: {:?}", e)),
    }
}

#[ic_cdk::update]
async fn mutate(
    signed_request: ICSigned<ICProxyMutateRequest>,
) -> Result<Option<ICProposalWithApprovals>, String> {
    let request = signed_request
        .parse(|i| match i {
            ICProxyMutateRequest::Propose { proposal } => *proposal.author_id,
            ICProxyMutateRequest::Approve { approval } => *approval.signer_id,
        })
        .map_err(|e| format!("Failed to verify signature: {}", e))?;

    match request {
        ICProxyMutateRequest::Propose { proposal } => internal_create_proposal(proposal).await,
        ICProxyMutateRequest::Approve { approval } => internal_approve_proposal(approval).await,
    }
}

async fn internal_approve_proposal(
    approval: ICProposalApprovalWithSigner,
) -> Result<Option<ICProposalWithApprovals>, String> {
    // Check membership
    if !check_member(approval.signer_id).await? {
        return Err("signer is not a member".to_string());
    }

    // First phase: Update approvals and check if we need to execute
    let should_execute = with_state_mut(|contract| {
        // Check if proposal exists
        if !contract.proposals.contains_key(&approval.proposal_id) {
            return Err("proposal does not exist".to_string());
        }

        let approvals = contract.approvals.entry(approval.proposal_id).or_default();

        if !approvals.insert(approval.signer_id) {
            return Err("proposal already approved".to_string());
        }

        Ok(approvals.len() as u32 >= contract.num_approvals)
    })?;

    // Execute if needed
    if should_execute {
        execute_proposal(&approval.proposal_id).await?;
    }

    // Build final response
    with_state(|contract| build_proposal_response(&*contract, approval.proposal_id))
}

async fn execute_proposal(proposal_id: &ProposalId) -> Result<(), String> {
    let proposal =
        remove_proposal(proposal_id).ok_or_else(|| "proposal does not exist".to_string())?;

    // Execute each action
    for action in proposal.actions {
        match action {
            ICProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit,
            } => {
                // If there's a deposit, transfer it first
                if deposit > 0 {
                    let ledger_id = with_state(|contract| contract.ledger_id.clone());

                    let transfer_args = TransferArgs {
                        memo: Memo(0),
                        amount: Tokens::from_e8s(
                            deposit
                                .try_into()
                                .map_err(|e| format!("Amount conversion error: {}", e))?,
                        ),
                        fee: Tokens::from_e8s(10_000), // Standard fee is 0.0001 ICP
                        from_subaccount: None,
                        to: AccountIdentifier::new(&receiver_id, &Subaccount([0; 32])),
                        created_at_time: None,
                    };

                    let _: (Result<u64, TransferError>,) =
                        ic_cdk::call(Principal::from(ledger_id), "transfer", (transfer_args,))
                            .await
                            .map_err(|e| format!("Transfer failed: {:?}", e))?;
                }

                // Then make the actual cross-contract call
                let args_bytes = candid::encode_one(args)
                    .map_err(|e| format!("Failed to encode args: {}", e))?;

                let _: () = ic_cdk::call(receiver_id, method_name.as_str(), (args_bytes,))
                    .await
                    .map_err(|e| format!("Inter-canister call failed: {:?}", e))?;
            }
            ICProposalAction::Transfer {
                receiver_id,
                amount,
            } => {
                let ledger_id = with_state(|contract| contract.ledger_id.clone());

                let transfer_args = TransferArgs {
                    memo: Memo(0),
                    amount: Tokens::from_e8s(
                        amount
                            .try_into()
                            .map_err(|e| format!("Amount conversion error: {}", e))?,
                    ),
                    fee: Tokens::from_e8s(10_000), // Standard fee is 0.0001 ICP
                    from_subaccount: None,
                    to: AccountIdentifier::new(&receiver_id, &Subaccount([0; 32])),
                    created_at_time: None,
                };

                let _: (Result<u64, TransferError>,) =
                    ic_cdk::call(Principal::from(ledger_id), "transfer", (transfer_args,))
                        .await
                        .map_err(|e| format!("Transfer failed: {:?}", e))?;
            }
            ICProposalAction::SetNumApprovals { num_approvals } => {
                with_state_mut(|contract| {
                    contract.num_approvals = num_approvals;
                });
            }
            ICProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => {
                with_state_mut(|contract| {
                    contract.active_proposals_limit = active_proposals_limit;
                });
            }
            ICProposalAction::SetContextValue { key, value } => {
                with_state_mut(|contract| {
                    contract.context_storage.insert(key, value);
                });
            }
            ICProposalAction::DeleteProposal { proposal_id: _ } => {}
        }
    }

    Ok(())
}

async fn internal_create_proposal(
    proposal: ICProposal,
) -> Result<Option<ICProposalWithApprovals>, String> {
    // Check membership
    if !check_member(proposal.author_id).await? {
        return Err("signer is not a member".to_string());
    }

    if proposal.actions.is_empty() {
        return Err("proposal cannot have empty actions".to_string());
    }

    // Check if the proposal contains a delete action
    for action in &proposal.actions {
        if let ICProposalAction::DeleteProposal { proposal_id } = action {
            // Get the proposal to be deleted
            let to_delete = with_state(|contract| contract.proposals.get(proposal_id).cloned())
                .ok_or("Proposal to delete does not exist")?;

            // Check if the current user is the author of the proposal to be deleted
            if to_delete.author_id != proposal.author_id {
                return Err("Only the proposal author can delete their proposals".to_string());
            }

            remove_proposal(proposal_id);
            return Ok(None);
        }
    }

    with_state_mut(|contract| {
        let num_proposals = contract
            .num_proposals_pk
            .get(&proposal.author_id)
            .copied()
            .unwrap_or(0);

        // Check proposal limit
        if num_proposals >= contract.active_proposals_limit {
            return Err(
                "Account has too many active proposals. Confirm or delete some.".to_string(),
            );
        }

        // Validate proposal actions
        for action in &proposal.actions {
            validate_proposal_action(action)?;
        }

        // Store proposal
        let proposal_id = proposal.id;
        let author_id = proposal.author_id;
        contract.proposals.insert(proposal_id, proposal);

        // Initialize approvals set with author's approval
        let approvals = BTreeSet::from([author_id]);
        contract.approvals.insert(proposal_id, approvals);

        // Update proposal count
        *contract.num_proposals_pk.entry(author_id).or_insert(0) += 1;

        build_proposal_response(&*contract, proposal_id)
    })
}

fn validate_proposal_action(action: &ICProposalAction) -> Result<(), String> {
    match action {
        ICProposalAction::ExternalFunctionCall {
            receiver_id: _,
            method_name,
            args: _,
            deposit: _,
        } => {
            if method_name.is_empty() {
                return Err("method name cannot be empty".to_string());
            }
        }
        ICProposalAction::Transfer {
            receiver_id: _,
            amount,
        } => {
            if *amount == 0 {
                return Err("transfer amount cannot be zero".to_string());
            }
        }
        ICProposalAction::SetNumApprovals { num_approvals } => {
            if *num_approvals == 0 {
                return Err("num approvals cannot be zero".to_string());
            }
        }
        ICProposalAction::SetActiveProposalsLimit {
            active_proposals_limit,
        } => {
            if *active_proposals_limit == 0 {
                return Err("active proposals limit cannot be zero".to_string());
            }
        }
        ICProposalAction::SetContextValue { .. } => {}
        ICProposalAction::DeleteProposal { .. } => {}
    }
    Ok(())
}

fn remove_proposal(proposal_id: &ProposalId) -> Option<ICProposal> {
    with_state_mut(|contract| {
        contract.approvals.remove(proposal_id);
        if let Some(proposal) = contract.proposals.remove(proposal_id) {
            let author_id = proposal.author_id;
            if let Some(count) = contract.num_proposals_pk.get_mut(&author_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    contract.num_proposals_pk.remove(&author_id);
                }
            }

            return Some(proposal);
        }

        None
    })
}

fn build_proposal_response(
    contract: &ICProxyContract,
    proposal_id: ICRepr<ProposalId>,
) -> Result<Option<ICProposalWithApprovals>, String> {
    let approvals = contract.approvals.get(&proposal_id);

    Ok(approvals.map(|approvals| ICProposalWithApprovals {
        proposal_id,
        num_approvals: approvals.len(),
    }))
}
