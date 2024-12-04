use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use candid::{CandidType, Principal};
use guard::Guard;
use serde::{Deserialize, Serialize};

use crate::types::{
    ICApplication, ICCapability, ICContextId, ICContextIdentity, ICPSigned, ICSignerId, Request,
};

pub mod guard;
pub mod mutate;
pub mod query;
pub mod sys;
pub mod types;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct Context {
    pub application: Guard<ICApplication>,
    pub members: Guard<Vec<ICContextIdentity>>,
    pub proxy: Guard<Principal>,
}

pub struct ContextConfigs {
    pub contexts: HashMap<ICContextId, Context>,
    pub proxy_code: Option<Vec<u8>>,
    pub owner: Principal,
    pub ledger_id: Principal,
}

impl Default for ContextConfigs {
    fn default() -> Self {
        Self {
            contexts: HashMap::new(),
            proxy_code: None,
            owner: ic_cdk::api::caller(),
            ledger_id: Principal::anonymous(),
        }
    }
}

thread_local! {
    pub static CONTEXT_CONFIGS: RefCell<ContextConfigs> = RefCell::new(ContextConfigs::default());
}

#[ic_cdk::init]
fn init() {
    CONTEXT_CONFIGS.with(|state| {
        *state.borrow_mut() = ContextConfigs::default();
    });
}

ic_cdk::export_candid!();
