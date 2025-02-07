#![cfg(test)]

use soroban_sdk::testutils::Address as _;
use soroban_sdk::token::{StellarAssetClient, TokenClient};
use soroban_sdk::{Address, Env, String};

use super::*;

#[test]
fn test_init_and_basic_storage() {
    let env = Env::default();
    env.mock_all_auths();

    // Create mock token
    let token_admin = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin.clone());

    let contract_id = env.register(MockExternalContract, (&token.address(),));
    let client = MockExternalContractClient::new(&env, &contract_id);

    // Test basic storage operation
    let key = String::from_str(&env, "test_key");
    let value = String::from_str(&env, "test_value");

    client.no_deposit(&key, &value);

    let stored = client.get_value(&key);
    assert_eq!(stored, Some(value));
}

#[test]
fn test_deposit_with_storage() {
    let env = Env::default();

    // Mock authorization
    env.mock_all_auths();

    // Create and setup token
    let token_admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_client = TokenClient::new(&env, &token.address());
    let token_asset_client = StellarAssetClient::new(&env, &token.address());

    let contract_id = env.register(MockExternalContract, (&token.address(),));
    let client = MockExternalContractClient::new(&env, &contract_id);

    // Mint some tokens to user
    token_asset_client.mint(&token_admin, &1000);
    token_asset_client.mint(&user, &1000);

    // Prepare deposit data
    let amount = 100;
    let key = String::from_str(&env, "deposit_key");
    let value = String::from_str(&env, "deposit_value");

    // Perform deposit
    let result = client
        .mock_all_auths_allowing_non_root_auth()
        .deposit(&user, &amount, &key, &value);
    assert_eq!(result, value);

    // Verify storage
    let stored = client.get_value(&key);
    assert_eq!(stored, Some(value));

    // Verify token transfer
    let contract_balance = token_client.balance(&contract_id);
    assert_eq!(contract_balance, amount);

    // Verify state
    let state = client.get_state();
    assert_eq!(state.total_deposits, amount);
}

#[test]
fn test_multiple_operations() {
    let env = Env::default();

    // Mock all authorization
    env.mock_all_auths();

    // Setup token
    let token_admin = Address::generate(&env);
    let user = Address::generate(&env);
    let token = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_client = TokenClient::new(&env, &token.address());
    let token_asset_client = StellarAssetClient::new(&env, &token.address());

    let contract_id = env.register(MockExternalContract, (&token.address(),));
    let client = MockExternalContractClient::new(&env, &contract_id);

    token_asset_client.mint(&token_admin, &1000);
    token_asset_client.mint(&user, &1000);

    // First deposit
    let result = client.mock_all_auths_allowing_non_root_auth().deposit(
        &user,
        &50,
        &String::from_str(&env, "key1"),
        &String::from_str(&env, "value1"),
    );
    assert_eq!(result, String::from_str(&env, "value1"));

    // Regular storage operation
    let result = client.no_deposit(
        &String::from_str(&env, "key2"),
        &String::from_str(&env, "value2"),
    );
    assert_eq!(result, String::from_str(&env, "value2"));

    // Second deposit
    let result = client.mock_all_auths_allowing_non_root_auth().deposit(
        &user,
        &30,
        &String::from_str(&env, "key3"),
        &String::from_str(&env, "value3"),
    );
    assert_eq!(result, String::from_str(&env, "value3"));

    // Verify all values
    assert_eq!(
        client.get_value(&String::from_str(&env, "key1")),
        Some(String::from_str(&env, "value1"))
    );
    assert_eq!(
        client.get_value(&String::from_str(&env, "key2")),
        Some(String::from_str(&env, "value2"))
    );
    assert_eq!(
        client.get_value(&String::from_str(&env, "key3")),
        Some(String::from_str(&env, "value3"))
    );

    // Verify final state
    let state = client.get_state();
    assert_eq!(state.total_deposits, 80);
    assert_eq!(token_client.balance(&contract_id), 80);
}
