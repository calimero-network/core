#![no_std]

use guard::Guard;
use soroban_sdk::{
    contract, contractimpl, symbol_short, Address, Env, Map, Symbol, BytesN, contracttype
};

mod guard;
mod mutate;
mod query;
mod sys;
mod types;

use types::Error;

const STORAGE_KEY_STATE: Symbol = symbol_short!("STATE");

#[contracttype]
#[derive(Clone)]
pub struct ContextConfigs {
    pub contexts: Map<BytesN<32>, Context>,
    pub proxy_code: OptionalBytes,
    pub owner: Address,
}

// Storage types
#[contracttype]
#[derive(Clone)]
pub struct Context {
    pub application: Guard,
    pub members: Guard,
    pub proxy: Guard,
    pub member_nonces: Map<BytesN<32>, u64>,
}

#[derive(Clone)]
#[contracttype]
pub enum OptionalBytes {
    Some(BytesN<32>),
    None,
}

// Helper methods for the custom option type
impl OptionalBytes {
  pub fn from_option(opt: Option<BytesN<32>>) -> Self {
      match opt {
          Some(bytes) => OptionalBytes::Some(bytes),
          None => OptionalBytes::None,
      }
  }

  pub fn to_option(&self) -> Option<BytesN<32>> {
      match self {
          OptionalBytes::Some(bytes) => Some(bytes.clone()),
          OptionalBytes::None => None,
      }
  }
}

#[contract]
pub struct ContextContract;

#[contractimpl]
impl ContextContract {
    pub fn initialize(env: Env, owner: Address) -> Result<(), Error> {
        // Require authorization from deployer
        owner.require_auth();

        if env.storage().instance().has(&symbol_short!("STATE")) {
            return Err(Error::Unauthorized);
        }

        let configs = ContextConfigs {
            contexts: Map::new(&env),
            proxy_code: OptionalBytes::None,
            owner,
        };

        env.storage().instance().set(&symbol_short!("STATE"), &configs);

        Ok(())
    }

    // Helper function to get state
    fn get_state(env: &Env) -> ContextConfigs {
        env.storage()
            .instance()
            .get(&STORAGE_KEY_STATE)
            .expect("not initialized")
    }

    // Helper function to save state
    fn save_state(env: &Env, state: &ContextConfigs) {
        env.storage().instance().set(&STORAGE_KEY_STATE, state);
    }

    // Helper function to update state
    fn update_state<F>(env: &Env, f: F) -> Result<(), Error>
    where
        F: FnOnce(&mut ContextConfigs) -> Result<(), Error>,
    {
        let mut state = Self::get_state(env);
        f(&mut state)?;
        Self::save_state(env, &state);
        Ok(())
    }
}

#[cfg(test)]
mod test;