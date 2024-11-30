use std::cell::RefCell;

use candid::Principal;

use crate::types::{
    ICPSigned, ICProposal, ICProposalApprovalWithSigner, ICProposalId, ICProposalWithApprovals,
    ICProxyContract, ICRequest, ICSignerId,
};

pub mod mutate;
pub mod query;
pub mod types;

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
