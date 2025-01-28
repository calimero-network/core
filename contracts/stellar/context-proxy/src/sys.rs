use soroban_sdk::{
  contractimpl, 
  Address, 
  BytesN,
  Env
};
use crate::{ContextProxyContract, StellarProxyError, ContextProxyContractClient, ContextProxyContractArgs};

#[contractimpl]
impl ContextProxyContract {
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

      // Verify the context address matches
      if context_address != state.context_config_id {
          return Err(StellarProxyError::Unauthorized);
      }

      // Deploy the upgrade
      env.deployer().update_current_contract_wasm(wasm_hash);
      
      Ok(())
  }
}
