use std::collections::HashSet;
use std::str::FromStr;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::SignerId;
use calimero_context_config::{
    ProposalAction, ProposalId, ProposalWithApprovals, ProxyMutateRequest,
};
use near_sdk::{
    env, near, require, AccountId, Gas, NearToken, Promise, PromiseError, PromiseOrValue,
    PromiseResult,
};

use super::{parse_input, Proposal, ProxyContract, ProxyContractExt, Signed};
use crate::ext_config::config_contract;
use crate::{assert_membership, MemberAction};

#[near]
impl ProxyContract {
    pub fn mutate(&mut self) -> Promise {
        if let (_, Some(_)) = self.code_size {
            env::panic_str("contract upgrade in progress");
        }

        parse_input!(request: Signed<ProxyMutateRequest>);

        let request = request
            .parse(|i| match i {
                ProxyMutateRequest::Propose { proposal } => *proposal.author_id,
                ProxyMutateRequest::Approve { approval } => *approval.signer_id,
            })
            .expect(&format!("Invalid input: {:?}", request));

        match request {
            ProxyMutateRequest::Propose { proposal } => self.propose(proposal),
            ProxyMutateRequest::Approve { approval } => {
                self.perform_action_by_member(MemberAction::Approve {
                    identity: approval.signer_id,
                    proposal_id: approval.proposal_id,
                })
            }
        }
    }
}
#[near]
impl ProxyContract {
    #[private]
    pub fn internal_approve_proposal(
        &mut self,
        signer_id: Repr<SignerId>,
        proposal_id: ProposalId,
        #[callback_result] call_result: Result<bool, PromiseError>, // Match the return type
    ) -> Option<ProposalWithApprovals> {
        assert_membership(call_result);

        self.internal_confirm(proposal_id, signer_id.rt().expect("Invalid signer"));
        self.build_proposal_response(proposal_id)
    }

    #[private]
    pub fn internal_create_proposal(
        &mut self,
        proposal: Proposal,
        num_proposals: u32,
        #[callback_result] call_result: Result<bool, PromiseError>, // Match the return type
    ) -> Option<ProposalWithApprovals> {
        assert_membership(call_result);

        self.num_proposals_pk
            .insert(*proposal.author_id, num_proposals);

        self.proposals.insert(proposal.id, proposal.clone());
        self.approvals.insert(proposal.id, HashSet::new());
        self.internal_confirm(
            proposal.id,
            proposal.author_id.rt().expect("Invalid signer"),
        );
        self.build_proposal_response(proposal.id)
    }

    #[private]
    pub fn finalize_execution(&mut self, proposal: Proposal) -> bool {
        let promise_count = env::promise_results_count();
        if promise_count > 0 {
            for i in 0..promise_count {
                match env::promise_result(i) {
                    PromiseResult::Successful(_) => continue,
                    _ => return false,
                }
            }
        }

        for action in proposal.actions {
            match action {
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit,
                } => {
                    self.active_proposals_limit = active_proposals_limit;
                }
                ProposalAction::SetNumApprovals { num_approvals } => {
                    self.num_approvals = num_approvals;
                }
                ProposalAction::SetContextValue { key, value } => {
                    self.internal_mutate_storage(key, value);
                }
                _ => {}
            }
        }
        true
    }

    fn execute_proposal(&mut self, proposal: Proposal) -> PromiseOrValue<bool> {
        let mut promise_actions = Vec::new();
        let mut non_promise_actions = Vec::new();

        for action in proposal.actions {
            match action {
                ProposalAction::ExternalFunctionCall { .. } | ProposalAction::Transfer { .. } => {
                    promise_actions.push(action)
                }
                _ => non_promise_actions.push(action),
            }
        }

        if promise_actions.is_empty() {
            self.finalize_execution(Proposal {
                id: proposal.id,
                author_id: proposal.author_id,
                actions: non_promise_actions,
            });
            return PromiseOrValue::Value(true);
        }

        let mut chained_promise: Option<Promise> = None;

        for action in promise_actions {
            let promise = match action {
                ProposalAction::ExternalFunctionCall {
                    receiver_id,
                    method_name,
                    args,
                    deposit,
                    gas,
                } => {
                    let account_id: AccountId =
                        AccountId::from_str(receiver_id.as_str()).expect("Invalid account ID");
                    Promise::new(account_id).function_call(
                        method_name,
                        args.into(),
                        NearToken::from_near(deposit),
                        Gas::from_gas(gas),
                    )
                }
                ProposalAction::Transfer {
                    receiver_id,
                    amount,
                } => {
                    let account_id: AccountId =
                        AccountId::from_str(receiver_id.as_str()).expect("Invalid account ID");
                    Promise::new(account_id).transfer(NearToken::from_near(amount))
                }
                _ => continue,
            };

            chained_promise = Some(match chained_promise {
                Some(accumulated) => accumulated.then(promise),
                None => promise,
            });
        }

        match chained_promise {
            Some(promise) => PromiseOrValue::Promise(promise.then(
                Self::ext(env::current_account_id()).finalize_execution(Proposal {
                    id: proposal.id,
                    author_id: proposal.author_id,
                    actions: non_promise_actions,
                }),
            )),
            None => PromiseOrValue::Value(true),
        }
    }

    #[private]
    pub fn internal_mutate_storage(
        &mut self,
        key: Box<[u8]>,
        value: Box<[u8]>,
    ) -> Option<Box<[u8]>> {
        self.context_storage.insert(key.clone(), value)
    }

    fn internal_confirm(&mut self, proposal_id: ProposalId, signer_id: SignerId) {
        let approvals = self.approvals.get_mut(&proposal_id).unwrap();
        assert!(
            !approvals.contains(&signer_id),
            "Already confirmed this proposal with this key"
        );
        if approvals.len() as u32 + 1 >= self.num_approvals {
            let proposal = self.remove_proposal(proposal_id);
            /********************************
            NOTE: If the tx execution fails for any reason, the proposals and approvals are removed already, so the client has to start all over
            ********************************/
            self.execute_proposal(proposal);
        } else {
            approvals.insert(signer_id);
        }
    }

    #[payable]
    pub fn update_contract(&mut self) -> Promise {
        require!(
            env::predecessor_account_id() == self.context_config_account_id,
            "Only the context config contract can update the proxy"
        );

        let new_code = env::input().expect("Expected proxy code");
        let (_, new_code_size @ None) = &mut self.code_size else {
            env::panic_str("contract upgrade in progress");
        };

        new_code_size.replace(new_code.len() as u64);

        Promise::new(env::current_account_id())
            .deploy_contract(new_code)
            .then(
                Self::ext(env::current_account_id())
                    .update_contract_callback(env::attached_deposit()),
            )
    }

    #[private]
    pub fn update_contract_callback(
        &mut self,
        attached_deposit: NearToken,
        #[callback_result] call_result: Result<(), PromiseError>,
    ) {
        let (old_code_size, Some(new_code_size)) = self.code_size else {
            env::panic_str("fatal: new code size not set");
        };

        self.code_size = (new_code_size, None);

        call_result.expect("Failed to update proxy contract");

        let old_cost = env::storage_byte_cost().saturating_mul(old_code_size as u128);
        let new_cost = env::storage_byte_cost().saturating_mul(new_code_size as u128);
        let refund = attached_deposit
            .saturating_add(old_cost)
            .saturating_sub(new_cost);

        Promise::new(self.context_config_account_id.clone()).transfer(refund);

        env::log_str("Successfully updated proxy contract code");
    }
}

impl ProxyContract {
    fn propose(&self, proposal: Proposal) -> Promise {
        require!(
            !self.proposals.contains_key(&proposal.id),
            "Proposal already exists"
        );
        let author_id = proposal.author_id;
        let num_proposals = self.num_proposals_pk.get(&author_id).unwrap_or(&0) + 1;
        assert!(
            num_proposals <= self.active_proposals_limit,
            "Account has too many active proposals. Confirm or delete some."
        );
        self.perform_action_by_member(MemberAction::Create {
            proposal,
            num_proposals,
        })
    }

    fn perform_action_by_member(&self, action: MemberAction) -> Promise {
        let identity = match &action {
            MemberAction::Approve { identity, .. } => identity,
            MemberAction::Create { proposal, .. } => &proposal.author_id,
        }
        .rt()
        .expect("Could not transmute");
        config_contract::ext(self.context_config_account_id.clone())
            .has_member(Repr::new(self.context_id), identity)
            .then(match action {
                MemberAction::Approve {
                    identity,
                    proposal_id,
                } => Self::ext(env::current_account_id())
                    .internal_approve_proposal(identity, proposal_id),
                MemberAction::Create {
                    proposal,
                    num_proposals,
                } => Self::ext(env::current_account_id())
                    .internal_create_proposal(proposal, num_proposals),
            })
    }

    fn build_proposal_response(&self, proposal_id: ProposalId) -> Option<ProposalWithApprovals> {
        let approvals = self.get_confirmations_count(proposal_id);
        match approvals {
            None => None,
            _ => Some(ProposalWithApprovals {
                proposal_id,
                num_approvals: approvals.unwrap().num_approvals,
            }),
        }
    }

    fn remove_proposal(&mut self, proposal_id: ProposalId) -> Proposal {
        self.approvals.remove(&proposal_id);
        let proposal = self
            .proposals
            .remove(&proposal_id)
            .expect("Failed to remove existing element");

        let author_id: SignerId = proposal.author_id.rt().expect("Invalid signer");
        let mut num_proposals = *self.num_proposals_pk.get(&author_id).unwrap_or(&0);

        num_proposals = num_proposals.saturating_sub(1);
        self.num_proposals_pk.insert(author_id, num_proposals);
        proposal
    }
}
