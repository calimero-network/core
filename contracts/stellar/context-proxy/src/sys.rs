use soroban_sdk::{contractimpl, Address, BytesN, Env};

use crate::{
    ContextProxyContract, ContextProxyContractArgs, ContextProxyContractClient, StellarProxyError,
};

#[contractimpl]
impl ContextProxyContract {
    /// Upgrades the proxy contract with new WASM code
    /// # Arguments
    /// * `wasm_hash` - Hash of the new WASM code to upgrade to
    /// * `context_address` - Address of the context configuration contract
    /// # Errors
    /// * Returns Unauthorized if caller is not the context configuration contract
    pub fn upgrade(
        env: Env,
        wasm_hash: BytesN<32>,
        context_address: Address,
    ) -> Result<(), StellarProxyError> {
        context_address.require_auth();

        // Get current state
        let state = Self::get_state(&env);

        // Check if caller is the context contract
        if context_address != state.context_config_id {
            return Err(StellarProxyError::Unauthorized);
        }

        // Deploy the upgrade
        env.deployer().update_current_contract_wasm(wasm_hash);

        Ok(())
    }
}
