use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use candid::{CandidType, Principal};
use serde::{Deserialize, Serialize};
use types::{ICContextId, LedgerId};

use crate::types::{
    ICPSigned, ICProposal, ICProposalApprovalWithSigner, ICProposalId, ICProposalWithApprovals,
    ICRequest, ICSignerId,
};

pub mod mutate;
pub mod query;
pub mod types;

#[derive(Serialize, Deserialize, Default)]
pub struct ICProxyContract {
    pub context_id: ICContextId,
    pub context_config_id: String,
    pub num_approvals: u32,
    pub proposals: BTreeMap<ICProposalId, ICProposal>,
    pub approvals: BTreeMap<ICProposalId, BTreeSet<ICSignerId>>,
    pub num_proposals_pk: BTreeMap<ICSignerId, u32>,
    pub active_proposals_limit: u32,
    pub context_storage: HashMap<Vec<u8>, Vec<u8>>,
    pub ledger_id: LedgerId,
}

impl ICProxyContract {
    pub fn new(context_id: ICContextId, ledger_id: Principal) -> Self {
        Self {
            context_id,
            context_config_id: ic_cdk::api::id().to_string(),
            num_approvals: 3,
            proposals: BTreeMap::new(),
            approvals: BTreeMap::new(),
            num_proposals_pk: BTreeMap::new(),
            active_proposals_limit: 10,
            context_storage: HashMap::new(),
            ledger_id: ledger_id.into(),
        }
    }
}

thread_local! {
  static PROXY_CONTRACT: RefCell<ICProxyContract> = RefCell::new(ICProxyContract::default());
}

#[ic_cdk::init]
fn init(context_id: types::ICContextId, ledger_id: Principal) {
    PROXY_CONTRACT.with(|contract| {
        *contract.borrow_mut() = ICProxyContract::new(context_id, ledger_id);
    });
}

ic_cdk::export_candid!();
