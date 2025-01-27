use calimero_context_config::stellar::stellar_types::StellarError;
use soroban_sdk::{contractimpl, log, Address, Bytes, BytesN, Env};

use crate::{ContextContract, ContextContractArgs, ContextContractClient, OptionalBytes};

#[contractimpl]
impl ContextContract {
    /// Upgrades the context contract with new WASM code
    /// # Arguments
    /// * `new_wasm` - The new WASM bytecode
    /// * `owner` - The contract owner's address
    /// # Errors
    /// Returns Unauthorized if caller is not the owner
    pub fn upgrade(env: &Env, new_wasm: Bytes, owner: Address) -> Result<(), StellarError> {
        // Verify authorization
        owner.require_auth();

        log!(env, "Upgrading context contract with new WASM");

        // Deploy the new WASM and get its hash
        let new_hash = env.deployer().upload_contract_wasm(new_wasm.clone());

        // Upgrade the contract to the new WASM
        env.deployer().update_current_contract_wasm(new_hash);
        Ok(())
    }

    /// Sets the proxy contract WASM code
    /// # Arguments
    /// * `proxy_wasm` - The proxy contract WASM bytecode
    /// * `owner` - The contract owner's address
    /// # Errors
    /// Returns Unauthorized if caller is not the owner
    pub fn set_proxy_code(
        env: &Env,
        proxy_wasm: Bytes,
        owner: Address,
    ) -> Result<BytesN<32>, StellarError> {

        let mut state = Self::get_state(env);
        if owner != state.owner {
            return Err(StellarError::Unauthorized);
        }

        // Verify authorization
        owner.require_auth();

        // Log before upload attempt
        log!(env, "Starting proxy WASM upload");

        // Upload WASM and get hash - this returns BytesN<32> directly
        let wasm_hash = env.deployer().upload_contract_wasm(proxy_wasm.clone());
        
        log!(env, "WASM upload successful, hash: {:?}", wasm_hash);

        // Update state with new hash
        state.proxy_code = OptionalBytes::from_option(Some(wasm_hash.clone()));
        Self::save_state(env, &state);

        Ok(wasm_hash)
    }
}
