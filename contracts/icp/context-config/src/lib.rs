use std::cell::RefCell;
use std::collections::BTreeMap;

use crate::types::ContextConfigs;
use crate::types::ICPSigned;
use crate::types::Request;
use crate::types::ICApplication;
use crate::types::ICCapability;
use crate::types::ICContextId;
use crate::types::ICContextIdentity;
use crate::types::ICSignerId;


pub mod guard;
pub mod mutate;
pub mod query;
pub mod types;

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