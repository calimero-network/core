use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{Application, ContextId, ContextIdentity, Signed, SignerId};
use calimero_context_config::{ContextRequest, ContextRequestKind, Request, RequestKind};
use ed25519_dalek::{Signer, SigningKey};
use eyre::Result;
use near_workspaces::network::Sandbox;
use near_workspaces::result::ExecutionFinalResult;
use near_workspaces::types::NearToken;
use near_workspaces::{Account, Contract, Worker};
use rand::Rng;
use serde_json::json;

use super::deploy_contract;

const CONTEXT_CONFIG_WASM: &str = "../context-config/res/calimero_context_config_near.wasm";

#[derive(Clone)]
pub struct ConfigContractHelper {
    pub config_contract: Contract,
}

impl ConfigContractHelper {
    pub async fn new(worker: &Worker<Sandbox>) -> Result<Self> {
        let config_contract = deploy_contract(worker, CONTEXT_CONFIG_WASM).await?;
        Ok(Self { config_contract })
    }

    pub async fn add_context_to_config(
        &self,
        caller: &Account,
        context: &SigningKey,
        author: &SigningKey,
    ) -> Result<ExecutionFinalResult> {
        let mut rng = rand::thread_rng();

        let application_id = rng.gen::<[_; 32]>().rt()?;
        let blob_id = rng.gen::<[_; 32]>().rt()?;

        let author_id: Repr<ContextIdentity> = Repr::new(author.verifying_key().rt()?);
        let context_id: Repr<ContextId> = Repr::new(context.verifying_key().rt()?);
        let context_signer: Repr<SignerId> = Repr::new(context.verifying_key().rt()?);

        let signed_request = Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::Add {
                        author_id,
                        application: Application::new(
                            application_id,
                            blob_id,
                            0,
                            Default::default(),
                            Default::default(),
                        ),
                    },
                ));
                Request::new(context_signer.rt()?, kind)
            },
            |p| context.sign(p),
        )?;
        let res = self.mutate_call(&caller, &signed_request).await?;
        Ok(res)
    }

    pub async fn update_proxy_contract(
        &self,
        caller: &Account,
        context_id: &SigningKey,
        host: &SigningKey,
    ) -> Result<ExecutionFinalResult> {
        let context_id: Repr<ContextId> = Repr::new(context_id.verifying_key().rt()?);
        let host_id: SignerId = host.verifying_key().rt()?;

        let signed_request = Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::UpdateProxyContract,
                ));

                Request::new(host_id.rt()?, kind)
            },
            |p| host.sign(p),
        )?;

        let res = caller
            .call(self.config_contract.id(), "mutate")
            .args_json(&signed_request)
            .max_gas()
            .transact()
            .await?;

        // Uncomment to print the result
        Ok(res)
    }

    pub async fn add_members(
        &self,
        caller: &Account,
        host: &SigningKey,
        guests: &[SigningKey],
        context: &SigningKey,
    ) -> Result<ExecutionFinalResult> {
        let guest_ids: Vec<Repr<ContextIdentity>> = guests
            .iter()
            .map(|x| Repr::new(x.verifying_key().rt().unwrap()))
            .collect();
        let host_id: Repr<ContextIdentity> = Repr::new(host.verifying_key().rt()?);
        let context_id: Repr<ContextId> = Repr::new(context.verifying_key().rt()?);

        let signed_request = Signed::new(
            &{
                let kind = RequestKind::Context(ContextRequest::new(
                    context_id,
                    ContextRequestKind::AddMembers {
                        members: guest_ids.into(),
                    },
                ));
                Request::new(host_id.rt()?, kind)
            },
            |p| host.sign(p),
        )?;

        let res = self.mutate_call(caller, &signed_request).await?;
        Ok(res)
    }

    async fn mutate_call<'a>(
        &'a self,
        caller: &'a Account,
        request: &'a Signed<Request<'a>>,
    ) -> Result<ExecutionFinalResult> {
        let res = caller
            .call(self.config_contract.id(), "mutate")
            .args_json(request)
            .max_gas()
            .transact()
            .await?;
        Ok(res)
    }

    pub async fn get_proxy_contract<'a>(
        &'a self,
        caller: &'a Account,
        context_id: &Repr<ContextId>,
    ) -> eyre::Result<Option<String>> {
        let res = caller
            .view(self.config_contract.id(), "proxy_contract")
            .args_json(json!({ "context_id": context_id }))
            .await?
            .json()?;
        Ok(res)
    }
}
