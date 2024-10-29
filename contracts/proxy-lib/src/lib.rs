use core::str;
use std::collections::HashSet;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{ContextId, Signed, SignerId};
use near_sdk::json_types::{Base64VecU8, U128};
use near_sdk::store::{IterableMap, LookupMap};
use near_sdk::{
    env, log, near, AccountId, Gas, NearToken, PanicOnDefault, Promise, PromiseError,
    PromiseOrValue,
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
        request_id: ProposalId,
    },
    Create {
        proposal: Proposal,
        num_proposals: u32,
    },
}
#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ProxyContract {
    pub context_id: Repr<ContextId>,
    pub context_config_account_id: AccountId,
    pub num_approvals: u32,
    pub proposal_nonce: ProposalId,
    pub proposals: IterableMap<ProposalId, Proposal>,
    pub approvals: IterableMap<ProposalId, HashSet<Repr<SignerId>>>,
    pub num_proposals_pk: IterableMap<SignerId, u32>,
    pub active_proposals_limit: u32,
    pub context_storage: LookupMap<Box<[u8]>, Box<[u8]>>,
    pub context_storage_keys: HashSet<Box<[u8]>>,
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
pub struct ConfirmationRequestWithSigner {
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

// The request the user makes specifying the receiving account and actions they want to execute (1 tx)
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
            context_id,
            context_config_account_id,
            proposal_nonce: 0,
            proposals: IterableMap::new(b"r".to_vec()),
            approvals: IterableMap::new(b"c".to_vec()),
            num_proposals_pk: IterableMap::new(b"k".to_vec()),
            num_approvals: 3,
            active_proposals_limit: 10,
            context_storage: LookupMap::new(b"l"),
            context_storage_keys: HashSet::new(),
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
        return self.perform_action_by_member(MemberAction::Create {
            proposal,
            num_proposals,
        });
    }

    pub fn approve(&mut self, request: Signed<ConfirmationRequestWithSigner>) -> Promise {
        let request = request
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");
        return self.perform_action_by_member(MemberAction::Approve {
            identity: request.signer_id,
            request_id: request.proposal_id,
        });
    }

    fn internal_confirm(&mut self, request_id: ProposalId, signer_id: Repr<SignerId>) -> () {
        let approvals = self.approvals.get_mut(&request_id).unwrap();
        assert!(
            !approvals.contains(&signer_id),
            "Already confirmed this request with this key"
        );
        if approvals.len() as u32 + 1 >= self.num_approvals {
            let request = self.remove_request(request_id);
            /********************************
            NOTE: If the tx execution fails for any reason, the request and confirmations are removed already, so the client has to start all over
            ********************************/
            self.execute_request(request);
        } else {
            approvals.insert(signer_id);
        }
    }

    fn perform_action_by_member(&self, action: MemberAction) -> Promise {
        let identity = match &action {
            MemberAction::Approve { identity, .. } => *identity,
            MemberAction::Create { proposal, .. } => proposal.author_id,
        };
        config_contract::ext(self.context_config_account_id.clone())
            .has_member(self.context_id, identity)
            .then(match action {
                MemberAction::Approve {
                    identity,
                    request_id,
                } => Self::ext(env::current_account_id())
                    .internal_approve_proposal(identity, request_id),
                MemberAction::Create {
                    proposal,
                    num_proposals,
                } => Self::ext(env::current_account_id())
                    .internal_create_proposal(proposal, num_proposals),
            })
    }

    pub fn requests(&self, offset: usize, length: usize) -> Vec<(&u32, &Proposal)> {
        let mut requests = Vec::with_capacity(length);
        for request in self.proposals.iter().skip(offset).take(length) {
            requests.push(request);
        }
        requests
    }

    pub fn get_confirmations_count(&self, proposal_id: ProposalId) -> ProposalWithApprovals {
        let size = self
            .approvals
            .get(&proposal_id)
            .unwrap_or(&HashSet::new())
            .len();
        ProposalWithApprovals {
            proposal_id,
            num_approvals: size,
        }
    }

    #[private]
    pub fn internal_approve_proposal(
        &mut self,
        signer_id: Repr<SignerId>,
        request_id: ProposalId,
        #[callback_result] call_result: Result<bool, PromiseError>, // Match the return type
    ) -> ProposalWithApprovals {
        assert_membership(call_result);

        self.internal_confirm(request_id, signer_id);
        return ProposalWithApprovals {
            proposal_id: request_id,
            num_approvals: self.get_confirmations_count(request_id).num_approvals,
        };
    }

    #[private]
    pub fn internal_create_proposal(
        &mut self,
        proposal: Proposal,
        num_proposals: u32,
        #[callback_result] call_result: Result<bool, PromiseError>, // Match the return type
    ) -> ProposalWithApprovals {
        assert_membership(call_result);

        self.num_proposals_pk
            .insert(*proposal.author_id, num_proposals);

        let proposal_id = self.proposal_nonce;

        self.proposals.insert(proposal_id, proposal.clone());
        self.approvals.insert(proposal_id, HashSet::new());
        self.internal_confirm(proposal_id, proposal.author_id);

        self.proposal_nonce += 1;

        return ProposalWithApprovals {
            proposal_id,
            num_approvals: self.get_confirmations_count(proposal_id).num_approvals,
        };
    }

    fn execute_request(&mut self, request: Proposal) -> PromiseOrValue<bool> {
        let mut result_promise: Option<Promise> = None;
        for action in request.actions {
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
                ProposalAction::SetActiveProposalsLimit {
                    active_proposals_limit,
                } => {
                    self.active_proposals_limit = active_proposals_limit;
                    return PromiseOrValue::Value(true);
                }
                ProposalAction::SetNumApprovals { num_approvals } => {
                    self.num_approvals = num_approvals;
                    return PromiseOrValue::Value(true);
                }
                ProposalAction::SetContextValue { key, value } => {
                    let value = self.internal_mutate_storage(key, value);
                    return PromiseOrValue::Value(value.is_some());
                }
            };
            if result_promise.is_none() {
                result_promise = Some(promise);
            } else {
                result_promise = Some(result_promise.unwrap().then(promise));
            }
        }
        if result_promise.is_none() {
            return PromiseOrValue::Value(false);
        }
        PromiseOrValue::Promise(result_promise.unwrap())
    }

    fn remove_request(&mut self, proposal_id: ProposalId) -> Proposal {
        self.approvals.remove(&proposal_id);
        let proposal = self
            .proposals
            .remove(&proposal_id)
            .expect("Failed to remove existing element");

        let author_id: SignerId = proposal.author_id.rt().expect("Invalid signer");
        let mut num_requests = *self.num_proposals_pk.get(&author_id).unwrap_or(&0);

        if num_requests > 0 {
            num_requests = num_requests - 1;
        }
        self.num_proposals_pk.insert(author_id, num_requests);
        proposal
    }

    #[private]
    pub fn internal_mutate_storage(
        &mut self,
        key: Box<[u8]>,
        value: Box<[u8]>,
    ) -> Option<Box<[u8]>> {
        let val = self.context_storage.insert(key.clone(), value);
        if val.is_some() {
            if !self.context_storage_keys.contains(&key) {
                self.context_storage_keys.insert(key);
            }
        }
        val
    }

    pub fn get_context_storage_keys(&self) -> HashSet<Box<[u8]>> {
        self.context_storage_keys.clone()
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
    assert!(call_result.is_ok(), "Membership check failed");
    assert!(call_result.unwrap(), "Not a context member");
    log!("Membership confirmed");
}
