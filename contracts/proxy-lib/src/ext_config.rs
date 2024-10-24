use near_sdk::ext_contract;
use calimero_context_config::repr::Repr;
use calimero_context_config::types::{ContextId, ContextIdentity};

#[ext_contract(config_contract)]
trait Configcontract {
    fn members(&self,
        context_id: Repr<ContextId> ,
        offset: usize,
        length: usize
    ) -> Vec<Repr<ContextIdentity>>;
}
