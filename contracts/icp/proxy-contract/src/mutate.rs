use std::collections::BTreeSet;

use calimero_context_config::repr::ReprTransmute;
use candid::Principal;
use ic_cdk::api::call::CallResult;
use ic_ledger_types::{AccountIdentifier, Memo, Subaccount, Tokens, TransferArgs, TransferError};

use crate::types::*;
use crate::{ICProxyContract, PROXY_CONTRACT};

async fn check_member(signer_id: &ICSignerId) -> Result<bool, String> {
    let (context_canister_id, context_id) = PROXY_CONTRACT.with(|contract| {
        (
            contract.borrow().context_config_id.clone(),
            contract.borrow().context_id.clone(),
        )
    });

    let identity = ICContextIdentity::new(signer_id.rt().expect("Invalid signer id"));

    let call_result: CallResult<(bool,)> = ic_cdk::call(
        Principal::from_text(&context_canister_id)
            .map_err(|e| format!("Invalid context canister ID: {}", e))?,
        "has_member",
        (context_id, identity),
    )
    .await;

    match call_result {
        Ok((is_member,)) => Ok(is_member),
        Err(e) => Err(format!("Error checking membership: {:?}", e)),
    }
}

#[ic_cdk::update]
async fn mutate(
    signed_request: ICPSigned<ICRequest>,
) -> Result<Option<ICProposalWithApprovals>, String> {
    let request = signed_request
        .parse(|r| r.signer_id)
        .map_err(|e| format!("Failed to verify signature: {}", e))?;

    // Check request timestamp
    let current_time = ic_cdk::api::time();
    if current_time.saturating_sub(request.timestamp_ms) > 1000 * 5 {
        return Err("request expired".to_string());
    }

    // Check membership
    if !check_member(&request.signer_id).await? {
        return Err("signer is not a member".to_string());
    }

    match request.kind {
        ICRequestKind::Propose { proposal } => internal_create_proposal(proposal),
        ICRequestKind::Approve { approval } => {
            internal_approve_proposal(
                approval.signer_id,
                approval.proposal_id,
                approval.added_timestamp,
            )
            .await
        }
    }
}

async fn internal_approve_proposal(
    signer_id: ICSignerId,
    proposal_id: ICProposalId,
    _added_timestamp: u64,
) -> Result<Option<ICProposalWithApprovals>, String> {
    // First phase: Update approvals and check if we need to execute
    let should_execute = PROXY_CONTRACT.with(|contract| {
        let mut contract = contract.borrow_mut();

        // Check if proposal exists
        if !contract.proposals.contains_key(&proposal_id) {
            return Err("proposal does not exist".to_string());
        }

        let approvals = contract.approvals.entry(proposal_id).or_default();

        if !approvals.insert(signer_id) {
            return Err("proposal already approved".to_string());
        }

        Ok(approvals.len() as u32 >= contract.num_approvals)
    })?;

    // Execute if needed
    if should_execute {
        execute_proposal(&proposal_id).await?;
    }

    // Build final response
    PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        build_proposal_response(&*contract, proposal_id)
    })
}

async fn execute_proposal(proposal_id: &ICProposalId) -> Result<(), String> {
    let proposal = PROXY_CONTRACT.with(|contract| {
        let contract = contract.borrow();
        contract
            .proposals
            .get(proposal_id)
            .cloned()
            .ok_or_else(|| "proposal does not exist".to_string())
    })?;

    // Execute each action
    for action in proposal.actions {
        match action {
            ICProposalAction::ExternalFunctionCall {
                receiver_id,
                method_name,
                args,
                deposit: _,
            } => {
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
                let ledger_id = PROXY_CONTRACT.with(|contract| contract.borrow().ledger_id.clone());

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
                PROXY_CONTRACT.with(|contract| {
                    let mut contract = contract.borrow_mut();
                    contract.num_approvals = num_approvals;
                });
            }
            ICProposalAction::SetActiveProposalsLimit {
                active_proposals_limit,
            } => {
                PROXY_CONTRACT.with(|contract| {
                    let mut contract = contract.borrow_mut();
                    contract.active_proposals_limit = active_proposals_limit;
                });
            }
            ICProposalAction::SetContextValue { key, value } => {
                PROXY_CONTRACT.with(|contract| {
                    let mut contract = contract.borrow_mut();
                    contract.context_storage.insert(key.clone(), value.clone());
                });
            }
        }
    }

    remove_proposal(proposal_id);
    Ok(())
}

fn internal_create_proposal(
    proposal: ICProposal,
) -> Result<Option<ICProposalWithApprovals>, String> {
    if proposal.actions.is_empty() {
        return Err("proposal cannot have empty actions".to_string());
    }

    PROXY_CONTRACT.with(|contract| {
        let mut contract = contract.borrow_mut();

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
    }
    Ok(())
}

fn remove_proposal(proposal_id: &ICProposalId) {
    PROXY_CONTRACT.with(|contract| {
        let mut contract = contract.borrow_mut();
        contract.approvals.remove(&proposal_id);
        if let Some(proposal) = contract.proposals.remove(&proposal_id) {
            let author_id = proposal.author_id;
            if let Some(count) = contract.num_proposals_pk.get_mut(&author_id) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    contract.num_proposals_pk.remove(&author_id);
                }
            }
        }
    });
}

fn build_proposal_response(
    contract: &ICProxyContract,
    proposal_id: ICProposalId,
) -> Result<Option<ICProposalWithApprovals>, String> {
    let approvals = contract.approvals.get(&proposal_id);

    Ok(approvals.map(|approvals| ICProposalWithApprovals {
        proposal_id,
        num_approvals: approvals.len(),
    }))
}
