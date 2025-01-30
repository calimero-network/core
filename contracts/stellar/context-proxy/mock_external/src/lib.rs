#![no_std]
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, log, symbol_short, token::TokenClient, Address, Env, Map, String, Symbol
};

const STORAGE_KEY: Symbol = symbol_short!("STATE");

#[contract]
pub struct MockExternalContract;

#[derive(Clone)]
#[contracttype]
pub struct MockExternalState {
    pub storage: Map<String, String>,
    pub total_deposits: i128,
    pub token: Address,
}

#[contracterror]
pub enum Error {
    InvalidAmount = 1,
    TransferFailed = 2,
}

#[contractimpl]
impl MockExternalContract {
    pub fn __constructor(env: Env, token: Address) {
        let state = MockExternalState {
            storage: Map::new(&env),
            total_deposits: 0,
            token,
        };
        env.storage().instance().set(&STORAGE_KEY, &state);
    }

    pub fn deposit(env: Env, from: Address, amount: i128, key: String, value: String) -> Result<String, Error> {
        from.require_auth();

        if amount <= 0 {
            return Err(Error::InvalidAmount);
        }

        let state: MockExternalState = env.storage().instance().get(&STORAGE_KEY).unwrap();
        let token_client = TokenClient::new(&env, &state.token);
        
        // Required token transfer
        token_client.transfer(&from, &env.current_contract_address(), &amount);

        let mut state: MockExternalState = env.storage().instance().get(&STORAGE_KEY).unwrap();
        state.total_deposits += amount;
        
        // Store the key-value pair
        state.storage.set(key.clone(), value.clone());
        
        env.storage().instance().set(&STORAGE_KEY, &state);

        Ok(value)
    }

    pub fn no_deposit(env: Env, key: String, value: String) -> Result<String, Error> {

        log!(&env, "No deposit");
        let mut state: MockExternalState = env.storage().instance().get(&STORAGE_KEY).unwrap();
        
        // Store the key-value pair
        state.storage.set(key.clone(), value.clone());

        // Check if there was a token transfer with this call
        let token_client = TokenClient::new(&env, &state.token);
        let contract_address = env.current_contract_address();
        let balance = token_client.balance(&contract_address);
        
        if balance > state.total_deposits {
            state.total_deposits = balance;
        }
        
        env.storage().instance().set(&STORAGE_KEY, &state);
        
        Ok(value)
    }

    pub fn get_value(env: Env, key: String) -> Option<String> {
        let state: MockExternalState = env.storage().instance().get(&STORAGE_KEY).unwrap();
        state.storage.get(key)
    }

    pub fn get_state(env: Env) -> MockExternalState {
        env.storage().instance().get(&STORAGE_KEY).unwrap()
    }
}

mod test;
