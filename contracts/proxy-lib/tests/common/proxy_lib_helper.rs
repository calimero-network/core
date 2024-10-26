use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{ContextId, Signed};
use ed25519_dalek::{Signer, SigningKey};
use eyre::Result;
use near_workspaces::result::ViewResultDetails;
use near_workspaces::{network::Sandbox, result::ExecutionFinalResult, Account, Contract, Worker};
use proxy_lib::{ConfirmationRequestWithSigner, Proposal, ProposalAction, ProposalId};
use serde_json::json;

use super::deploy_contract;

const PROXY_CONTRACT_WASM: &str = "./res/proxy_lib.wasm";

pub struct ProxyContractHelper {
    proxy_contract: Contract,
    config_contract: Contract,
}

impl ProxyContractHelper {
    pub async fn new(worker: &Worker<Sandbox>, config_contract: Contract) -> Result<Self> {
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
    ) -> Result<ExecutionFinalResult, near_workspaces::error::Error> {
        caller
            .call(self.proxy_contract.id(), "init")
            .args_json(json!({
                "context_id": context_id,
                "context_config_account_id": self.config_contract.id(),
            }))
            .transact()
            .await
    }

    pub fn create_proposal(
        &self,
        author: &SigningKey,
        receiver: &Account,
        actions: Vec<ProposalAction>,
    ) -> Result<Signed<Proposal>> {
        let proposal = Proposal {
            receiver_id: receiver.id().clone(),
            author_id: author.verifying_key().rt().expect("Invalid signer"),
            actions
        };
        let signed = Signed::new(&proposal, |p| author.sign(p))?;
        Ok(signed)
    }

    pub async fn create_and_approve_proposal(
        &self,
        caller: &Account,
        proposal: &Signed<Proposal>,
    ) -> Result<ExecutionFinalResult> {
        let call = caller
            .call(self.proxy_contract.id(), "create_and_approve_proposal")
            .args_json(json!({
                "proposal": proposal
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
    ) -> Result<ExecutionFinalResult> {
        let signer_id = signer
            .verifying_key()
            .to_bytes()
            .rt()
            .expect("Invalid signer");
        let args = Signed::new(
            &{
                ConfirmationRequestWithSigner {
                    signer_id,
                    proposal_id: proposal_id.clone(),
                    added_timestamp: 0,
                }
            },
            |p| signer.sign(p),
        )
        .expect("Failed to sign proposal");
        let res = caller
            .call(self.proxy_contract.id(), "approve")
            .args_json(json!({"request": args}))
            .max_gas()
            .transact()
            .await?;
        Ok(res)
    }

    pub async fn view_proposal_confirmations(
        &self,
        caller: &Account,
        proposal_id: &ProposalId,
    ) -> Result<ViewResultDetails> {
        let res = caller
            .view(self.proxy_contract.id(), "get_confirmations_count")
            .args_json(json!({ "proposal_id": proposal_id }))
            .await?;
        Ok(res)
    }
}
