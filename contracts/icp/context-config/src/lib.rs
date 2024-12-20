use std::cell::RefCell;
use std::collections::BTreeMap;

use calimero_context_config::icp::repr::ICRepr;
use calimero_context_config::icp::types::{ICApplication, ICCapability, ICRequest, ICSigned};
use calimero_context_config::types::{ContextId, ContextIdentity, SignerId};
use candid::{CandidType, Principal};
use serde::Deserialize;

mod guard;
mod mutate;
mod query;
mod sys;

use guard::Guard;

thread_local! {
    pub static CONTEXT_CONFIGS: RefCell<Option<ContextConfigs>> = RefCell::new(None);
}

#[derive(CandidType, Deserialize, Debug)]
pub struct Context {
    pub application: Guard<ICApplication>,
    pub members: Guard<BTreeMap<ICRepr<ContextIdentity>, u64>>,
    pub proxy: Guard<Principal>,
}

#[derive(CandidType, Deserialize, Debug)]
pub struct ContextConfigs {
    pub contexts: BTreeMap<ICRepr<ContextId>, Context>,
    pub proxy_code: Option<Vec<u8>>,
    pub owner: Principal,
    pub ledger_id: Principal,
}

#[ic_cdk::init]
fn init() {
    CONTEXT_CONFIGS.with(|state| {
        *state.borrow_mut() = Some(ContextConfigs {
            contexts: BTreeMap::new(),
            proxy_code: None,
            owner: ic_cdk::api::caller(),
            ledger_id: Principal::anonymous(),
        });
    });
}

fn with_state<F, R>(f: F) -> R
where
    F: FnOnce(&ContextConfigs) -> R,
{
    CONTEXT_CONFIGS.with(|configs| {
        let configs = configs.borrow();
        f(configs.as_ref().expect("cannister is being upgraded"))
    })
}

fn with_state_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut ContextConfigs) -> R,
{
    CONTEXT_CONFIGS.with(|configs| {
        let mut configs = configs.borrow_mut();
        f(configs.as_mut().expect("cannister is being upgraded"))
    })
}

ic_cdk::export_candid!();
