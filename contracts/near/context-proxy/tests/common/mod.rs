use ed25519_dalek::SigningKey;
use eyre::Result;
use near_workspaces::network::Sandbox;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract, Worker};
use rand::Rng;

pub mod config_helper;
pub mod counter_helper;
pub mod proxy_lib_helper;

pub async fn deploy_contract(worker: &Worker<Sandbox>, wasm_path: &str) -> Result<Contract> {
    let wasm = std::fs::read(wasm_path)?;
    let contract = worker.dev_deploy(&wasm).await?;
    Ok(contract)
}

pub fn generate_keypair() -> Result<SigningKey> {
    let mut rng = rand::thread_rng();
    let sk = SigningKey::from_bytes(&rng.gen());
    Ok(sk)
}

pub async fn create_account_with_balance(
    worker: &Worker<Sandbox>,
    prefix: &str,
    balance: u128,
) -> Result<Account> {
    let random_suffix: u32 = rand::thread_rng().gen_range(0..999999);

    // Take first 8 chars of prefix and combine with random number
    let prefix = prefix.chars().take(8).collect::<String>();
    let account_id = format!("{}{}", prefix, random_suffix);

    let root_account = worker.root_account()?;
    let account = root_account
        .create_subaccount(&account_id)
        .initial_balance(NearToken::from_near(balance))
        .transact()
        .await?
        .into_result()?;
    Ok(account)
}
