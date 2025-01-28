#![no_std]
use calimero_context_config::stellar::{StellarProposal, StellarProxyError};
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env, Map, Symbol,
    Vec,
};

mod mutate;
mod query;
mod sys;

#[derive(Clone)]
#[contracttype]
pub struct ProxyState {
    pub context_id: BytesN<32>,
    pub context_config_id: Address,
    pub num_approvals: u32,
    pub proposals: Map<BytesN<32>, StellarProposal>,
    pub approvals: Map<BytesN<32>, Vec<BytesN<32>>>, // proposal_id -> Vec<signer_id>
    pub num_proposals_pk: Map<BytesN<32>, u32>,      // author_id -> count
    pub active_proposals_limit: u32,
    pub context_storage: Map<Bytes, Bytes>,
    pub ledger_id: Address,
}

const STORAGE_KEY_STATE: Symbol = symbol_short!("STATE");

#[contract]
pub struct ContextProxyContract;

#[contractimpl]
impl ContextProxyContract {
    pub fn __constructor(
        env: Env,
        context_id: BytesN<32>,
        owner: Address,
        ledger_id: Address,
    ) -> Result<(), StellarProxyError> {
        // owner.require_auth();

        // Check if already initialized
        if env.storage().instance().has(&STORAGE_KEY_STATE) {
            return Err(StellarProxyError::AlreadyInitialized);
        }

        // Initialize contract state
        let state = ProxyState {
            context_id,
            context_config_id: owner.clone(),
            num_approvals: 3,
            proposals: Map::new(&env),
            approvals: Map::new(&env),
            num_proposals_pk: Map::new(&env),
            active_proposals_limit: 10,
            context_storage: Map::new(&env),
            ledger_id,
        };

        // Save state
        env.storage().instance().set(&STORAGE_KEY_STATE, &state);
        Ok(())
    }

    // Helper function to get state
    fn get_state(env: &Env) -> ProxyState {
        env.storage()
            .instance()
            .get(&STORAGE_KEY_STATE)
            .expect("Contract state not initialized")
    }

    // Helper function to save state
    fn save_state(env: &Env, state: &ProxyState) {
        env.storage().instance().set(&STORAGE_KEY_STATE, state);
    }
}

#[cfg(test)]
mod test;
