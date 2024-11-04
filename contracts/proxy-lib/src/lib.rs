use core::str;
use std::collections::HashSet;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{ContextId, Signed, SignerId};
use calimero_context_config::{Proposal, ProposalId, ProposalWithApprovals};
use near_sdk::json_types::U128;
use near_sdk::store::IterableMap;
use near_sdk::{near, AccountId, PanicOnDefault, PromiseError};

pub mod ext_config;
mod mutate;
pub use crate::ext_config::config_contract;

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

#[near]
impl ProxyContract {
    #[init]
    pub fn init(context_id: Repr<ContextId>, context_config_account_id: AccountId) -> Self {
        Self {
            context_id: context_id.rt().expect("Invalid context id"),
            context_config_account_id,
            proposals: IterableMap::new(b"r".to_vec()),
            approvals: IterableMap::new(b"c".to_vec()),
            num_proposals_pk: IterableMap::new(b"k".to_vec()),
            num_approvals: 3,
            active_proposals_limit: 10,
            context_storage: IterableMap::new(b"l"),
        }
    }

    pub fn proposals(&self, offset: usize, length: usize) -> Vec<&Proposal> {
        let effective_len = (self.proposals.len() as usize)
            .saturating_sub(offset)
            .min(length);
        let mut proposals = Vec::with_capacity(effective_len);
        for proposal in self.proposals.iter().skip(offset).take(length) {
            proposals.push(proposal.1);
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
