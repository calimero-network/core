use calimero_context_config::repr::Repr;
use calimero_context_config::types::{ContextId, SignerId};
use near_sdk::ext_contract;

#[ext_contract(config_contract)]
pub trait ConfigContract {
    fn has_member(&self, context_id: Repr<ContextId>, identity: Repr<SignerId>) -> bool;
}
