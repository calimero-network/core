use calimero_context_config::{{repr::Repr, repr::ReprTransmute}, types::ContextIdentity};
use ed25519_dalek::SigningKey;
use eyre::Result;
use near_workspaces::{network::Sandbox, types::NearToken, Account, Contract, Worker};
use rand::Rng;

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

pub async fn create_account_with_balance(worker: &Worker<Sandbox>, account_id: &str, balance: u128) -> Result<Account> {
    let root_account = worker.dev_create_account().await?;
    let account = root_account
        .create_subaccount(account_id)
        .initial_balance(NearToken::from_near(balance))
        .transact()
        .await?
        .into_result()?;
    Ok(account)
}

pub async fn create_subaccount(worker: &Worker<Sandbox>, subaccount_id: &str) -> Result<Account> {
    let root_account = worker.root_account()?;
    let subaccount = root_account
        .create_subaccount(subaccount_id)
        .transact()
        .await?
        .into_result()?;
    Ok(subaccount)
}
