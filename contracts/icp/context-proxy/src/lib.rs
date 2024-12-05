use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::types::ICSigned;
use calimero_context_config::icp::{
    ICProposal, ICProposalApprovalWithSigner, ICProposalWithApprovals, ICProxyMutateRequest,
};
use calimero_context_config::types::{ContextId, ProposalId, SignerId};
use candid::{CandidType, Principal};
use serde::Deserialize;

mod mutate;
mod query;
mod sys;

thread_local! {
  static PROXY_CONTRACT: RefCell<Option<ICProxyContract>> = RefCell::new(None);
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ICProxyContract {
    pub context_id: ICRepr<ContextId>,
    pub context_config_id: Principal,
    pub num_approvals: u32,
    pub proposals: BTreeMap<ICRepr<ProposalId>, ICProposal>,
    pub approvals: BTreeMap<ICRepr<ProposalId>, BTreeSet<ICRepr<SignerId>>>,
    pub num_proposals_pk: BTreeMap<ICRepr<SignerId>, u32>,
    pub active_proposals_limit: u32,
    pub context_storage: HashMap<Vec<u8>, Vec<u8>>,
    pub ledger_id: Principal,
}

#[ic_cdk::init]
fn init(context_id: ICRepr<ContextId>, ledger_id: Principal) {
    PROXY_CONTRACT.with(|contract| {
        *contract.borrow_mut() = Some(ICProxyContract {
            context_id,
            context_config_id: ic_cdk::caller(),
            num_approvals: 3,
            proposals: BTreeMap::new(),
            approvals: BTreeMap::new(),
            num_proposals_pk: BTreeMap::new(),
            active_proposals_limit: 10,
            context_storage: HashMap::new(),
            ledger_id,
        });
    });
}

fn with_state<F, R>(f: F) -> R
where
    F: FnOnce(&ICProxyContract) -> R,
{
    PROXY_CONTRACT.with(|state| {
        let state = state.borrow();
        f(state.as_ref().expect("cannister is being upgraded"))
    })
}

fn with_state_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut ICProxyContract) -> R,
{
    PROXY_CONTRACT.with(|state| {
        let mut state = state.borrow_mut();
        f(state.as_mut().expect("cannister is being upgraded"))
    })
}

ic_cdk::export_candid!();
