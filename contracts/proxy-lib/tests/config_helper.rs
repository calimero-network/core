#[path = "test_utils.rs"]
mod test_utils;

use calimero_context_config::repr::{Repr, ReprTransmute};
use calimero_context_config::types::{Application, ContextId, ContextIdentity, Signed, SignerId};
use calimero_context_config::{ContextRequest, ContextRequestKind, Request, RequestKind};
use ed25519_dalek::{Signer, SigningKey};
use eyre::Result;
use near_workspaces::{network::Sandbox, result::ExecutionFinalResult, Account, Contract, Worker};
use rand::Rng;
use test_utils::deploy_contract;

const CONTEXT_CONFIG_WASM: &str = "../context-config/res/calimero_context_config_near.wasm";

#[derive(Clone)]
pub struct ConfigContractHelper {
    pub config_contract: Contract,
}

impl ConfigContractHelper {
    pub async fn new(worker: &Worker<Sandbox>) -> Result<Self> {
        let config_contract = deploy_contract(worker, CONTEXT_CONFIG_WASM).await?;
        Ok(Self {
            config_contract,
        })
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

        let author_id: Repr<ContextIdentity> = Repr::new(author.verifying_key().to_bytes().rt()?);
        let context_id: Repr<ContextId> = Repr::new(context.verifying_key().to_bytes().rt()?);
        let context_signer: Repr<SignerId> = Repr::new(context.verifying_key().to_bytes().rt()?);

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

    pub async fn add_members(
        &self,
        caller: &Account,
        host: &SigningKey,
        guests: &[SigningKey],
        context: &SigningKey,
    ) -> Result<ExecutionFinalResult> {
        let guest_ids: Vec<Repr<ContextIdentity>> = guests.iter()
            .map(|x| Repr::new(x.verifying_key().to_bytes().rt().unwrap()))
            .collect();
        let host_id: Repr<ContextIdentity> = Repr::new(host.verifying_key().to_bytes().rt()?);
        let context_id: Repr<ContextId> = Repr::new(context.verifying_key().to_bytes().rt()?);

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

    async fn mutate_call<'a>(&'a self, caller: &'a Account, request: &'a Signed<Request<'a>>) -> Result<ExecutionFinalResult> {
        let res = caller
            .call(self.config_contract.id(), "mutate")
            .args_json(request)
            .transact()
            .await?;
        Ok(res)
    }
}
