use std::{cell::RefCell, collections::HashMap};
use std::collections::BTreeMap;

use candid::CandidType;
use guard::Guard;
use serde::{Deserialize, Serialize};

use crate::types::{
    ICApplication, ICCapability, ICContextId, ICContextIdentity, ICPSigned, ICSignerId, Request,
};

pub mod guard;
pub mod mutate;
pub mod query;
pub mod types;

#[derive(CandidType, Serialize, Deserialize, Clone, Debug)]
pub struct Context {
    pub application: Guard<ICApplication>,
    pub members: Guard<Vec<ICContextIdentity>>,
    pub proxy: Guard<String>,
}

pub struct ContextConfigs {
    pub contexts: HashMap<ICContextId, Context>,
    pub next_proxy_id: u64,
}

impl Default for ContextConfigs {
    fn default() -> Self {
        Self {
            contexts: HashMap::new(),
            next_proxy_id: 0,
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
