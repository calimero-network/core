#![allow(unused_crate_dependencies, reason = "False positives")]

use near_workspaces::network::Sandbox;
use near_workspaces::{Contract, Worker};

const CONTRACT_WASM: &str = "./res/calimero_test_counter_near.wasm";

async fn deploy_counter_contract(worker: &Worker<Sandbox>) -> eyre::Result<Contract> {
    let wasm = std::fs::read(CONTRACT_WASM)?;
    let contract = worker.dev_deploy(&wasm).await?;
    Ok(contract)
}

#[tokio::test]
async fn test_counter_contract() -> eyre::Result<()> {
    let worker = near_workspaces::sandbox().await?;
    let counter_contract = deploy_counter_contract(&worker).await?;

    let _res = counter_contract
        .call("new")
        .transact()
        .await?
        .into_result()?;

    let _res = counter_contract
        .call("increment")
        .transact()
        .await?
        .into_result()?;

    let counter_value: u32 = counter_contract.view("get_count").await?.json()?;

    assert_eq!(counter_value, 1, "Counter should be incremented once");

    Ok(())
}
