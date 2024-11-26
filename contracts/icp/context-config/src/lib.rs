use std::cell::RefCell;
use crate::types::ContextConfigs;

pub mod guard;
pub mod types;
pub mod mutate;

thread_local! {
    pub static CONTEXT_CONFIGS: RefCell<ContextConfigs> = RefCell::new(ContextConfigs::default());
}

#[ic_cdk::init]
fn init() {
    CONTEXT_CONFIGS.with(|state| {
        *state.borrow_mut() = ContextConfigs::default();
    });
}
