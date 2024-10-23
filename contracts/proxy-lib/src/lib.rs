use near_sdk::{env, log, near, AccountId, Gas, PanicOnDefault, Promise, PromiseError};
use calimero_context_config::repr::Repr;
use calimero_context_config::types::{ContextId, ContextIdentity};

pub mod ext_config;
pub use crate::ext_config::config_contract;

#[near(contract_state)]
#[derive(PanicOnDefault)]
pub struct ProxyContract {
    pub context_id: Repr<ContextId>,
    pub context_config_account_id: AccountId,
}

#[near]
impl ProxyContract {
    #[init]
    pub fn init(context_id: Repr<ContextId>, context_config_account_id: AccountId) -> Self {
        assert!(!env::state_exists(), "Already initialized");
        Self {
            context_id, 
            context_config_account_id 
        }
    }
    
    pub fn fetch_members(
        &self,
    ) -> Promise {
        log!("Starting fetch_members...");
        config_contract::ext(self.context_config_account_id.clone())
            .with_static_gas(Gas::from_tgas(5))
            .members(self.context_id, 0, 10)
            .then(
               Self::ext(env::current_account_id()).internal_process_members()
            )
    }

    #[private]
    pub fn internal_process_members(
        &mut self,
        #[callback_result] call_result: Result<Vec<Repr<ContextIdentity>>, PromiseError>,  // Match the return type
    ) -> Vec<Repr<ContextIdentity>> {
        if call_result.is_err() {
            log!("fetch_members failed...");
            return [].to_vec();
        } else {
            log!("fetch_members was successful!");
            return call_result.unwrap();
        }
    }
}
