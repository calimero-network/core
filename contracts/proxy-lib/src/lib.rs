use core::str;
use std::collections::HashSet;

use calimero_context_config::repr::Repr;
use calimero_context_config::types::{ContextId, Signed, SignerId};
use near_sdk::json_types::{Base64VecU8, U128, U64};
use near_sdk::store::IterableMap;
use near_sdk::{env, log, near, AccountId, Gas, PanicOnDefault, Promise, PromiseError};

pub mod ext_config;
pub use crate::ext_config::config_contract;

pub type ProposalId = u32;

#[derive(PartialEq, Debug)]
#[near(serializers = [json, borsh])]
pub struct ProposalWithApprovals {
    pub proposal_id: ProposalId,
    pub num_approvals: usize,
}

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ProxyContract {
    pub context_id: Repr<ContextId>,
    pub context_config_account_id: AccountId,
    pub num_approvals: u32,
    pub proposal_nonce: ProposalId,
    pub proposals: IterableMap<ProposalId, ProposalWithSigner>,
    pub approvals: IterableMap<ProposalId, HashSet<Repr<SignerId>>>,
    pub num_proposals_pk: IterableMap<Repr<SignerId>, u32>,
    pub active_proposals_limit: u32,
}

#[derive(Clone, Debug)]
#[near(serializers = [borsh])]
pub struct FunctionCallPermission {
    allowance: Option<U128>,
    receiver_id: AccountId,
    method_names: Vec<String>,
}

// An internal request wrapped with the signer_pk and added timestamp to determine num_requests_pk and prevent against malicious key holder gas attacks
#[derive(Clone, PartialEq, Debug)]
#[near(serializers = [json, borsh])]
pub struct ProposalWithSigner {
    pub proposal: Proposal,
    pub signer_id: Repr<SignerId>,
}

// An internal request wrapped with the signer_pk and added timestamp to determine num_requests_pk and prevent against malicious key holder gas attacks
#[derive(Clone, PartialEq)]
#[near(serializers = [json, borsh])]
pub struct ConfirmationRequestWithSigner {
    pub proposal_id: ProposalId,
    pub signer_id: Repr<SignerId>,
    pub added_timestamp: u64,
}

/// Lowest level action that can be performed by the multisig contract.
#[derive(Clone, PartialEq, Debug)]
#[near(serializers = [json, borsh])]
pub enum ProposalAction {
    FunctionCall {
        method_name: String,
        args: Base64VecU8,
        deposit: U128,
        gas: U64,
    },
}

// The request the user makes specifying the receiving account and actions they want to execute (1 tx)
#[derive(Clone, PartialEq, Debug)]
#[near(serializers = [json, borsh])]
pub struct Proposal {
    pub receiver_id: AccountId,
    pub actions: Vec<ProposalAction>,
}

#[near]
impl ProxyContract {
    #[init]
    pub fn init(context_id: Repr<ContextId>, context_config_account_id: AccountId) -> Self {
        assert!(!env::state_exists(), "Already initialized");
        Self {
            context_id,
            context_config_account_id,
            proposal_nonce: 0,
            proposals: IterableMap::new(b"r".to_vec()),
            approvals: IterableMap::new(b"c".to_vec()),
            num_proposals_pk: IterableMap::new(b"k".to_vec()),
            num_approvals: 2,
            active_proposals_limit: 10,
        }
    }

    pub fn create_proposal(&mut self, proposal: Signed<ProposalWithSigner>) -> ProposalId {
        // Verify the signature corresponds to the signer_id
        let proposal = proposal
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");

        let singer_id = &proposal.signer_id;

        let num_proposals = self.num_proposals_pk.get(singer_id).unwrap_or(&0) + 1;
        assert!(
            num_proposals <= self.active_proposals_limit,
            "Account has too many active proposals. Confirm or delete some."
        );
        self.num_proposals_pk
            .insert(singer_id.clone(), num_proposals);

        self.proposals.insert(self.proposal_nonce, proposal.clone());
        self.approvals.insert(self.proposal_nonce, HashSet::new());
        self.proposal_nonce += 1;
        self.proposal_nonce - 1
    }

    pub fn create_and_approve_proposal(&mut self, proposal: Signed<ProposalWithSigner>) -> Promise {
        let proposal_id = self.create_proposal(proposal.clone());
        let proposal = proposal
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");
        log!("Starting approval...");
        return self.approve_by_member(proposal.signer_id, proposal_id);
    }

    pub fn approve(&mut self, request: Signed<ConfirmationRequestWithSigner>) -> Promise {
        let request = request
            .parse(|i| *i.signer_id)
            .expect("failed to parse input");
        return self.approve_by_member(request.signer_id, request.proposal_id);
    }

    fn internal_confirm(&mut self, request_id: ProposalId, signer_id: Repr<SignerId>) -> () {
        let confirmations = self.approvals.get_mut(&request_id).unwrap();
        assert!(
            !confirmations.contains(&signer_id),
            "Already confirmed this request with this key"
        );
        confirmations.insert(signer_id);

        // Check if the number of confirmations is enough to execute the request
        // If so, execute the request
        // If not, do nothing
    }

    fn approve_by_member(&self, identity: Repr<SignerId>, request_id: ProposalId) -> Promise {
        log!("Starting fetch_members...");
        config_contract::ext(self.context_config_account_id.clone())
            .with_static_gas(Gas::from_tgas(5))
            .has_member(self.context_id, identity)
            .then(
                Self::ext(env::current_account_id()).internal_process_members(identity, request_id),
            )
    }

    pub fn requests(&self, offset: usize, length: usize) -> Vec<(&u32, &ProposalWithSigner)> {
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
    pub fn internal_process_members(
        &mut self,
        signer_id: Repr<SignerId>,
        request_id: ProposalId,
        #[callback_result] call_result: Result<bool, PromiseError>, // Match the return type
    ) -> ProposalWithApprovals {
        assert!(call_result.is_ok(), "Error: Membership check failed");
        assert!(call_result.unwrap(), "Error: Is not a member");
        log!("Success: Membership confirmed");
        self.internal_confirm(request_id, signer_id);
        return ProposalWithApprovals {
            proposal_id: request_id,
            num_approvals: self.get_confirmations_count(request_id).num_approvals,
        };
    }
}
