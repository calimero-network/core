#![allow(unused_crate_dependencies)]

use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract};
use serde_json::{json, Value};
use tokio::fs::read as async_read;

#[tokio::test]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let worker = near_workspaces::sandbox().await?;
    let wasm = async_read("res/calimero_registry.wasm").await?;
    let contract = worker.dev_deploy(&wasm).await?;

    println!("Contract deployed at: {}", contract.id());

    // create accounts
    let account = worker.dev_create_account().await?;
    let bobo = account
        .create_subaccount("bobo")
        .initial_balance(NearToken::from_near(30))
        .transact()
        .await?
        .into_result()?;

    // begin tests
    test_add_package_and_release(&bobo, &contract).await?;
    Ok(())
}

async fn test_add_package_and_release(
    user: &Account,
    contract: &Contract,
) -> Result<(), Box<dyn std::error::Error>> {
    let package_id: Value = user
        .call(contract.id(), "add_package")
        .args_json(json!({
            "name": "application",
            "description": "Demo Application",
            "repository": "https://github.com/application",
        }))
        .transact()
        .await?
        .json()?;

    let package: Value = user
        .view(contract.id(), "get_package")
        .args_json(json!({
            "id": package_id,
        }))
        .await?
        .json()?;

    assert_eq!(package["name"], "application".to_string());
    assert_eq!(package["owner"], user.id().to_string());

    drop(
        user.call(contract.id(), "add_release")
            .args_json(json!({
                "name": "application",
                "version": "0.1.0",
                "notes": "",
                "path": "https://gateway/ipfs/CID",
                "hash": "123456789",
            }))
            .transact()
            .await?,
    );

    let release: Value = user
        .view(contract.id(), "get_release")
        .args_json(json!({
            "id": package_id,
            "version": "0.1.0",
        }))
        .await?
        .json()?;

    assert_eq!(release["version"], "0.1.0".to_string());

    Ok(())
}
