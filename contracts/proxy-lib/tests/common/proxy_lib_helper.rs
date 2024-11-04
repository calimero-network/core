use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{ContextId, Signed};
use calimero_context_config::{
    Proposal, ProposalAction, ProposalApprovalWithSigner, ProposalId, ProxyMutateRequest,
};
use ed25519_dalek::{Signer, SigningKey};
use near_workspaces::network::Sandbox;
use near_workspaces::result::{ExecutionFinalResult, ViewResultDetails};
use near_workspaces::{Account, Contract, Worker};
use serde_json::json;

use super::deploy_contract;

const PROXY_CONTRACT_WASM: &str = "./res/proxy_lib.wasm";

pub struct ProxyContractHelper {
    pub proxy_contract: Contract,
    config_contract: Contract,
}

impl ProxyContractHelper {
    pub async fn new(worker: &Worker<Sandbox>, config_contract: Contract) -> eyre::Result<Self> {
        let proxy_contract = deploy_contract(worker, PROXY_CONTRACT_WASM).await?;
        Ok(Self {
            proxy_contract,
            config_contract,
        })
    }

    pub async fn initialize(
        &self,
        caller: &Account,
        context_id: &Repr<ContextId>,
    ) -> eyre::Result<ExecutionFinalResult, near_workspaces::error::Error> {
        caller
            .call(self.proxy_contract.id(), "init")
            .args_json(json!({
                "context_id": context_id,
                "context_config_account_id": self.config_contract.id(),
            }))
            .transact()
            .await
    }

    pub fn create_proposal_request(
        &self,
        id: &ProposalId,
        author: &SigningKey,
        actions: &Vec<ProposalAction>,
    ) -> eyre::Result<Signed<ProxyMutateRequest>> {
        let request = ProxyMutateRequest::Propose {
            proposal: Proposal {
                id: id.clone(),
                author_id: author.verifying_key().rt().expect("Invalid signer"),
                actions: actions.clone(),
            },
        };
        let signed = Signed::new(&request, |p| author.sign(p))?;
        Ok(signed)
    }

    pub async fn proxy_mutate(
        &self,
        caller: &Account,
        request: &Signed<ProxyMutateRequest>,
    ) -> eyre::Result<ExecutionFinalResult> {
        let call = caller
            .call(self.proxy_contract.id(), "mutate")
            .args_json(json!({
                "request": request
            }))
            .max_gas()
            .transact()
            .await?;
        Ok(call)
    }

    pub async fn approve_proposal(
        &self,
        caller: &Account,
        signer: &SigningKey,
        proposal_id: &ProposalId,
    ) -> eyre::Result<ExecutionFinalResult> {
        let signer_id = signer
            .verifying_key()
            .to_bytes()
            .rt()
            .expect("Invalid signer");

        let request = ProxyMutateRequest::Approve {
            approval: ProposalApprovalWithSigner {
                signer_id,
                proposal_id: proposal_id.clone(),
                added_timestamp: 0,
            },
        };
        let signed_request = Signed::new(&request, |p| signer.sign(p))?;
        let res = caller
            .call(self.proxy_contract.id(), "mutate")
            .args_json(json!({"request": signed_request}))
            .max_gas()
            .transact()
            .await?;
        Ok(res)
    }

    pub async fn view_proposal_confirmations(
        &self,
        caller: &Account,
        proposal_id: &ProposalId,
    ) -> eyre::Result<ViewResultDetails> {
        let res = caller
            .view(self.proxy_contract.id(), "get_confirmations_count")
            .args_json(json!({ "proposal_id": proposal_id }))
            .await?;
        Ok(res)
    }

    pub async fn view_active_proposals_limit(&self, caller: &Account) -> eyre::Result<u32> {
        let res: u32 = caller
            .view(self.proxy_contract.id(), "get_active_proposals_limit")
            .await?
            .json()?;
        Ok(res)
    }

    pub async fn view_num_approvals(&self, caller: &Account) -> eyre::Result<u32> {
        let res: u32 = caller
            .view(self.proxy_contract.id(), "get_num_approvals")
            .await?
            .json()?;
        Ok(res)
    }

    pub async fn view_context_value(
        &self,
        caller: &Account,
        key: Box<[u8]>,
    ) -> eyre::Result<Option<Box<[u8]>>> {
        let res: Option<Box<[u8]>> = caller
            .view(self.proxy_contract.id(), "get_context_value")
            .args_json(json!({ "key": key }))
            .await?
            .json()?;
        Ok(res)
    }

    pub async fn view_proposals(
        &self,
        caller: &Account,
        offset: usize,
        length: usize,
    ) -> eyre::Result<Vec<Proposal>> {
        let res = caller
            .view(self.proxy_contract.id(), "proposals")
            .args_json(json!({ "offset": offset, "length": length }))
            .await?
            .json()?;
        Ok(res)
    }

    pub async fn view_proposal(
        &self,
        caller: &Account,
        id: &ProposalId,
    ) -> eyre::Result<Option<Proposal>> {
        let res = caller
            .view(self.proxy_contract.id(), "proposal")
            .args_json(json!({ "proposal_id": id }))
            .await?
            .json()?;
        Ok(res)
    }
}
