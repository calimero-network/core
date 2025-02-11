#![no_std]
use calimero_context_config::stellar::{StellarProposal, StellarProxyError};
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, Address, Bytes, BytesN, Env, Map, Symbol,
    Vec,
};

mod mutate;
mod query;
mod sys;

/// State of the proxy contract containing all context-related data
#[derive(Clone)]
#[contracttype]
pub struct ProxyState {
    /// ID of the context this proxy belongs to
    pub context_id: BytesN<32>,
    /// Address of the context configuration contract
    pub context_config_id: Address,
    /// Number of approvals required for proposal execution
    pub num_approvals: u32,
    /// Map of active proposals indexed by their IDs
    pub proposals: Map<BytesN<32>, StellarProposal>,
    /// Map of proposal approvals: proposal_id -> Vec<signer_id>
    pub approvals: Map<BytesN<32>, Vec<BytesN<32>>>,
    /// Map tracking number of active proposals per author: author_id -> count
    pub num_proposals_pk: Map<BytesN<32>, u32>,
    /// Maximum number of active proposals allowed per author
    pub active_proposals_limit: u32,
    /// Storage for context-specific key-value pairs
    pub context_storage: Map<Bytes, Bytes>,
    /// Address of the Stellar ledger token contract
    pub ledger_id: Address,
}

/// Key used to store the contract state
const STORAGE_KEY_STATE: Symbol = symbol_short!("STATE");

#[contract]
pub struct ContextProxyContract;

#[contractimpl]
impl ContextProxyContract {
    /// Initializes a new proxy contract instance
    /// # Arguments
    /// * `context_id` - ID of the context this proxy belongs to
    /// * `owner` - Address of the context configuration contract
    /// * `ledger_id` - Address of the Stellar ledger token contract
    /// # Errors
    /// Returns AlreadyInitialized if the contract has already been initialized
    pub fn __constructor(
        env: Env,
        context_id: BytesN<32>,
        owner: Address,
        ledger_id: Address,
    ) -> Result<(), StellarProxyError> {
        if env.storage().instance().has(&STORAGE_KEY_STATE) {
            return Err(StellarProxyError::AlreadyInitialized);
        }

        let state = ProxyState {
            context_id,
            context_config_id: owner,
            num_approvals: 3,
            proposals: Map::new(&env),
            approvals: Map::new(&env),
            num_proposals_pk: Map::new(&env),
            active_proposals_limit: 10,
            context_storage: Map::new(&env),
            ledger_id,
        };

        env.storage().instance().set(&STORAGE_KEY_STATE, &state);
        Ok(())
    }

    /// Retrieves the current contract state
    /// # Panics
    /// Panics if the contract state is not initialized
    fn get_state(env: &Env) -> ProxyState {
        env.storage()
            .instance()
            .get(&STORAGE_KEY_STATE)
            .expect("Contract state not initialized")
    }

    /// Saves the contract state
    fn save_state(env: &Env, state: &ProxyState) {
        env.storage().instance().set(&STORAGE_KEY_STATE, state);
    }
}
