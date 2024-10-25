use near_sdk::ext_contract;
use calimero_context_config::repr::Repr;
use calimero_context_config::types::{SignerId, ContextId};

#[ext_contract(config_contract)]
trait Configcontract {
    fn has_member(&self,
        context_id: Repr<ContextId>,
        identity: Repr<SignerId>
    ) -> bool;
}
