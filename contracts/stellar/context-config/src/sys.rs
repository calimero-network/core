use soroban_sdk::{
    log, Address, Bytes, BytesN, Env, contractimpl
};

use crate::Error;
use crate::ContextContract;
use crate::ContextContractClient;
use crate::ContextContractArgs;
use crate::OptionalBytes;

#[contractimpl]
impl ContextContract {
    /// Upgrades the context contract with new WASM code
    /// # Arguments
    /// * `new_wasm` - The new WASM bytecode
    /// * `owner` - The contract owner's address
    /// # Errors
    /// Returns Unauthorized if caller is not the owner
    pub fn upgrade(
        env: &Env, 
        new_wasm: Bytes, 
        owner: Address
    ) -> Result<(), Error> {
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
        owner: Address
    ) -> Result<BytesN<32>, Error> {
        owner.require_auth();        
        
        let mut state = Self::get_state(env);
        if owner != state.owner {
            return Err(Error::Unauthorized);
        }
        log!(&env, "Uploading proxy WASM");
        
        // Upload WASM and get hash
        let wasm_hash = env.deployer().upload_contract_wasm(proxy_wasm.clone());
        log!(&env, "Generated WASM hash: {:?}", wasm_hash);
        
        // Store the hash in state
        state.proxy_code = OptionalBytes::from_option(Some(wasm_hash.clone()));
        Self::save_state(env, &state);

        Ok(wasm_hash)
    }
}
