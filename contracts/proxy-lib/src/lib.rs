use core::str;
use std::collections::HashSet;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{ContextId, Signed, SignerId};
use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::store::IterableMap;
use near_sdk::{
    env, near, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError, PromiseOrValue,
    PromiseResult,
};

pub mod ext_config;
pub use crate::ext_config::config_contract;

pub type ProposalId = u32;

#[derive(PartialEq, Debug)]
#[near(serializers = [json, borsh])]
pub struct ProposalWithApprovals {
    pub proposal_id: ProposalId,
    pub num_approvals: usize,
}

enum MemberAction {
    Approve {
        identity: Repr<SignerId>,
        proposal_id: ProposalId,
    },
    Create {
        proposal: Proposal,
        num_proposals: u32,
    },
}
#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ProxyContract {
    pub context_id: ContextId,
    pub context_config_account_id: AccountId,
    pub num_approvals: u32,
    pub proposal_nonce: ProposalId,
    pub proposals: IterableMap<ProposalId, Proposal>,
    pub approvals: IterableMap<ProposalId, HashSet<SignerId>>,
    pub num_proposals_pk: IterableMap<SignerId, u32>,
    pub active_proposals_limit: u32,
    pub context_storage: IterableMap<Box<[u8]>, Box<[u8]>>,
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh])]
pub struct FunctionCallPermission {
    allowance: Option<U128>,
    receiver_id: AccountId,
    method_names: Vec<String>,
}

#[derive(Clone, PartialEq)]
#[near(serializers = [json, borsh])]
pub struct ProposalApprovalWithSigner {
    pub proposal_id: ProposalId,
    pub signer_id: Repr<SignerId>,
    pub added_timestamp: u64,
}

#[derive(Clone, PartialEq, Debug)]
#[near(serializers = [json, borsh])]
pub enum ProposalAction {
    ExternalFunctionCall {
        receiver_id: AccountId,
        method_name: String,
        args: Base64VecU8,
        deposit: NearToken,
        gas: Gas,
    },
    Transfer {
        receiver_id: AccountId,
        amount: NearToken,
    },
    SetNumApprovals {
        num_approvals: u32,
    },
    SetActiveProposalsLimit {
        active_proposals_limit: u32,
    },
    SetContextValue {
        key: Box<[u8]>,
        value: Box<[u8]>,
    },
}

// The proposal the user makes specifying the receiving account and actions they want to execute (1 tx)
#[derive(Clone, PartialEq, Debug)]
#[near(serializers = [json, borsh])]
pub struct Proposal {
    pub author_id: Repr<SignerId>,
    pub actions: Vec<ProposalAction>,
}

#[near]
impl ProxyContract {
    #[init]
    pub fn init(context_id: Repr<ContextId>, context_config_account_id: AccountId) -> Self {
        Self {
            context_id: context_id.rt().expect("Invalid context id"),
            context_config_account_id,
            proposal_nonce: 0,
            proposals: IterableMap::new(b"r".to_vec()),
            approvals: IterableMap::new(b"c".to_vec()),
            num_proposals_pk: IterableMap::new(b"k".to_vec()),
            num_approvals: 3,
            active_proposals_limit: 10,
            context_storage: IterableMap::new(b"l"),
        }
    }

    pub fn create_and_approve_proposal(&self, proposal: Signed<Proposal>) -> Promise {
        // Verify the signature corresponds to the signer_id
        let proposal = proposal
            .parse(|i| *i.author_id)
            .expect("failed to parse input");

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

    pub fn approve(&mut self, proposal: Signed<ProposalApprovalWithSigner>) -> Promise {
        let proposal = proposal
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");
        self.perform_action_by_member(MemberAction::Approve {
            identity: proposal.signer_id,
            proposal_id: proposal.proposal_id,
        })
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

    pub fn proposals(&self, offset: usize, length: usize) -> Vec<(&u32, &Proposal)> {
        let effective_len = (self.proposals.len() as usize)
            .saturating_sub(offset)
            .min(length);
        let mut proposals = Vec::with_capacity(effective_len);
        for proposal in self.proposals.iter().skip(offset).take(length) {
            proposals.push(proposal);
        }
        proposals
    }

    pub fn get_confirmations_count(
        &self,
        proposal_id: ProposalId,
    ) -> Option<ProposalWithApprovals> {
        let approvals_for_proposal = self.approvals.get(&proposal_id);
        approvals_for_proposal.map(|approvals| ProposalWithApprovals {
            proposal_id,
            num_approvals: approvals.len(),
        })
    }

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

        let proposal_id = self.proposal_nonce;

        self.proposals.insert(proposal_id, proposal.clone());
        self.approvals.insert(proposal_id, HashSet::new());
        self.internal_confirm(
            proposal_id,
            proposal.author_id.rt().expect("Invalid signer"),
        );

        self.proposal_nonce += 1;

        self.build_proposal_response(proposal_id)
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
                    Promise::new(receiver_id).function_call(method_name, args.into(), deposit, gas)
                }
                ProposalAction::Transfer {
                    receiver_id,
                    amount,
                } => Promise::new(receiver_id).transfer(amount),
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
                    author_id: proposal.author_id,
                    actions: non_promise_actions,
                }),
            )),
            None => PromiseOrValue::Value(true),
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

    #[private]
    pub fn internal_mutate_storage(
        &mut self,
        key: Box<[u8]>,
        value: Box<[u8]>,
    ) -> Option<Box<[u8]>> {
        self.context_storage.insert(key.clone(), value)
    }

    #[expect(clippy::type_complexity, reason = "Acceptable here")]
    pub fn context_storage_entries(
        &self,
        offset: usize,
        length: usize,
    ) -> Vec<(&Box<[u8]>, &Box<[u8]>)> {
        let effective_len = (self.context_storage.len() as usize)
            .saturating_sub(offset)
            .min(length);
        let mut context_storage_entries = Vec::with_capacity(effective_len);
        for entry in self.context_storage.iter().skip(offset).take(length) {
            context_storage_entries.push(entry);
        }
        context_storage_entries
    }

    pub fn get_context_value(&self, key: Box<[u8]>) -> Option<Box<[u8]>> {
        self.context_storage.get(&key).cloned()
    }

    pub fn get_num_approvals(&self) -> u32 {
        self.num_approvals
    }

    pub fn get_active_proposals_limit(&self) -> u32 {
        self.active_proposals_limit
    }
}

fn assert_membership(call_result: Result<bool, PromiseError>) {
    let has_member = call_result.expect("Membership check failed");
    assert!(has_member, "Is not a member");
}
