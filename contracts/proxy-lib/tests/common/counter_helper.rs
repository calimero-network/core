use eyre::Result;
use near_workspaces::{network::Sandbox, Contract, Worker};

use super::deploy_contract;

const COUNTER_WASM: &str = "../test-counter/res/test_counter_near.wasm";

#[derive(Clone)]
pub struct CounterContractHelper {
    pub counter_contract: Contract,
}

impl CounterContractHelper {
    pub async fn deploy_and_initialize(worker: &Worker<Sandbox>) -> Result<Self> {
        let counter_contract = deploy_contract(worker, COUNTER_WASM).await?;

        let _res = counter_contract
            .call("new")
            .transact()
            .await?
            .into_result()?; 
        Ok(Self {
            counter_contract,
        })
    }

    pub async fn get_value(&self) -> Result<u32> {
        let counter_value: u32 = self.counter_contract
            .view("get_count")
            .await?
            .json()?;
        Ok(counter_value)
    }
}
