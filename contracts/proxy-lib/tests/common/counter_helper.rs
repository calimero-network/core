use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{Application, ContextId, ContextIdentity, Signed, SignerId};
use calimero_context_config::{ContextRequest, ContextRequestKind, Request, RequestKind};
use ed25519_dalek::{Signer, SigningKey};
use eyre::Result;
use near_workspaces::{network::Sandbox, result::ExecutionFinalResult, Account, Contract, Worker};
use rand::Rng;

use super::deploy_contract;

const COUNTER_WASM: &str = "../test-counter/res/test_counter_near.wasm";

#[derive(Clone)]
pub struct CounterContracttHelper {
    pub counter_contract: Contract,
}

impl CounterContracttHelper {
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

    pub async fn get_valuer(&self) -> Result<u32> {
        let counter_value: u32 = self.counter_contract
            .view("get_count")
            .await?
            .json()?;
        Ok(counter_value)
    }
}
