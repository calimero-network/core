use std::collections::HashSet;

use crate::types::*;
use crate::PROXY_CONTRACT;
use candid::Principal;

// Helper function to convert Identity to Principal
fn identity_to_principal(identity: &Identity) -> Principal {
    Principal::from_slice(&identity[..])
}

async fn check_member(_signer_id: &ICSignerId) -> Result<bool, String> {
    // let context_canister_id = PROXY_CONTRACT.with(|contract| {
    //     contract.borrow().context_config_id.clone()
    // });

    // let principal = identity_to_principal(&signer_id.0);
    // TODO: implement this
    // let call_result: CallResult<(bool,)> = ic_cdk::call(
    //     Principal::from_text(&context_canister_id)
    //         .map_err(|e| format!("Invalid context canister ID: {}", e))?,
    //     "is_member",
    //     (principal,),
    // ).await.map_err(|e| format!("Failed to call context contract: {:?}", e))?;

    Ok(true)
}

#[ic_cdk::update]
async fn mutate(signed_request: ICPSigned<ICRequest>) -> Result<Option<ICProposalWithApprovals>, String> {
    let request = signed_request
        .parse(|r| &r.signer_id)
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

    match &request.kind {
        ICRequestKind::Propose { proposal } => {
            let num_proposals = PROXY_CONTRACT.with(|contract| {
                let contract = contract.borrow();
                contract
                    .num_proposals_pk
                    .get(&proposal.author_id)
                    .copied()
                    .unwrap_or(0)
            });

            internal_create_proposal(proposal.clone(), num_proposals)
        }
        ICRequestKind::Approve { approval } => {
            internal_approve_proposal(
                approval.signer_id.clone(),
                approval.proposal_id.clone(),
                approval.added_timestamp,
            ).await
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

        let approvals = contract.approvals.entry(proposal_id.clone()).or_default();
        
        if approvals.contains(&signer_id) {
            return Err("proposal already approved".to_string());
        }

        approvals.insert(signer_id);

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
        contract.proposals.get(proposal_id)
            .cloned()
            .ok_or_else(|| "proposal does not exist".to_string())
    })?;

    // Execute each action
    for action in &proposal.actions {
        match action {
            ICProposalAction::ExternalFunctionCall { receiver_id, method_name, args, deposit: _ } => {
                let receiver = identity_to_principal(receiver_id);

                let args_bytes = hex::decode(args)
                    .map_err(|e| format!("Invalid args hex encoding: {}", e))?;

                let _: ((),) = ic_cdk::call(
                    receiver,
                    method_name,
                    (args_bytes,),
                )
                .await
                .map_err(|e| format!("Inter-canister call failed: {:?}", e))?;
            }
            ICProposalAction::Transfer { receiver_id, amount } => {
                let ledger_id = "ryjl3-tyaaa-aaaaa-aaaba-cai";
                let receiver = identity_to_principal(receiver_id);
                
                let transfer_args = LedgerTransferArgs {
                    to: receiver.to_string(),
                    amount: *amount,
                };

                let _: ((),) = ic_cdk::call(
                    Principal::from_text(ledger_id)
                        .map_err(|e| format!("Invalid ledger ID: {}", e))?,
                    "transfer",
                    (transfer_args,),
                )
                .await
                .map_err(|e| format!("Transfer failed: {:?}", e))?;
            }
            ICProposalAction::SetNumApprovals { num_approvals } => {
                PROXY_CONTRACT.with(|contract| {
                    let mut contract = contract.borrow_mut();
                    contract.num_approvals = *num_approvals;
                });
            }
            ICProposalAction::SetActiveProposalsLimit { active_proposals_limit } => {
                PROXY_CONTRACT.with(|contract| {
                    let mut contract = contract.borrow_mut();
                    contract.active_proposals_limit = *active_proposals_limit;
                });
            }
            ICProposalAction::SetContextValue { key, value } => {
                if PROXY_CONTRACT.with(|contract| contract.borrow().code_size.1.is_some()) {
                    return Err("contract upgrade in progress".to_string());
                }
                PROXY_CONTRACT.with(|contract| {
                    let mut contract = contract.borrow_mut();
                    contract.context_storage.insert(key.clone(), value.clone());
                });
            }
        }
    }

    remove_proposal(proposal_id.clone());
    Ok(())
}

fn internal_create_proposal(
    proposal: ICProposal,
    num_proposals: u32,
) -> Result<Option<ICProposalWithApprovals>, String> {
    PROXY_CONTRACT.with(|contract| {
        let mut contract = contract.borrow_mut();
          
        // Check proposal limit
        if num_proposals >= contract.active_proposals_limit {
            return Err("Account has too many active proposals. Confirm or delete some.".to_string());
        }

        // Validate proposal actions
        for action in &proposal.actions {
            validate_proposal_action(action)?;
        }

        // Store proposal
        let proposal_id = proposal.id.clone();
        contract.proposals.insert(proposal_id.clone(), proposal.clone());
        
        // Initialize approvals set with author's approval
        let mut approvals = HashSet::new();
        approvals.insert(proposal.author_id.clone());
        contract.approvals.insert(proposal_id.clone(), approvals);
        
        // Update proposal count
        let author_id = proposal.author_id;
        *contract.num_proposals_pk.entry(author_id).or_insert(0) += 1;

        build_proposal_response(&*contract, proposal_id)
    })
}

fn validate_proposal_action(action: &ICProposalAction) -> Result<(), String> {
    match action {
        ICProposalAction::ExternalFunctionCall { receiver_id, method_name, args, deposit: _ } => {
            if method_name.is_empty() {
                return Err("method name cannot be empty".to_string());
            }
            if args.is_empty() {
                return Err("args cannot be empty".to_string());
            }
            // Just convert to Principal, no need for ? operator
            identity_to_principal(receiver_id);
        }
        ICProposalAction::Transfer { receiver_id, amount } => {
            if *amount == 0 {
                return Err("transfer amount cannot be zero".to_string());
            }
            // Just convert to Principal, no need for ? operator
            identity_to_principal(receiver_id);
            
            if *amount > 1_000_000_000 {
                return Err("transfer amount limit exceeded".to_string());
            }
        }
        ICProposalAction::SetNumApprovals { num_approvals } => {
            if *num_approvals == 0 {
                return Err("num approvals cannot be zero".to_string());
            }
        }
        ICProposalAction::SetActiveProposalsLimit { active_proposals_limit } => {
            if *active_proposals_limit == 0 {
                return Err("active proposals limit cannot be zero".to_string());
            }
        }
        ICProposalAction::SetContextValue { key, value } => {
            if key.is_empty() {
                return Err("key cannot be empty".to_string());
            }
            if value.is_empty() {
                return Err("value cannot be empty".to_string());
            }
        }
    }
    Ok(())
}

fn remove_proposal(proposal_id: ICProposalId) {
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