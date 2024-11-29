
use crate::types::{
  ICProxyContract,
  ICProposalId,
  ICProposal,
  ICProposalWithApprovals,
  ICPSigned,
  ICRequest,
  ICSignerId,
  ICProposalApprovalWithSigner,
};

use std::cell::RefCell;


pub mod types;
pub mod mutate;
pub mod query;

thread_local! {
  static PROXY_CONTRACT: RefCell<ICProxyContract> = RefCell::new(ICProxyContract::default());
}

#[ic_cdk::init]
fn init(context_id: types::ICContextId) {
  PROXY_CONTRACT.with(|contract| {
      *contract.borrow_mut() = ICProxyContract::new(context_id);
  });
}

ic_cdk::export_candid!();

