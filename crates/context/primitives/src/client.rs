use actix::Recipient;
use async_stream::try_stream;
use calimero_context_config::client::env::config::ContextConfig as ContextConfigEnv;
use calimero_context_config::client::{AnyTransport, Client as ExternalClient};
use calimero_context_config::repr::ReprTransmute;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::key::{
    ContextConfig as ContextConfigKey, ContextIdentity as ContextIdentityKey,
    ContextMeta as ContextMetaKey,
};
use calimero_store::types::ContextIdentity as ContextIdentityValue;
use calimero_store::{key, Store};
use calimero_utils_actix::LazyRecipient;
use eyre::OptionExt;
use futures_util::Stream;
use tokio::sync::oneshot;

use crate::messages::create_context::{CreateContextRequest, CreateContextResponse};
use crate::messages::execute::{ExecuteError, ExecuteRequest, ExecuteResponse};
use crate::messages::update_application::UpdateApplicationRequest;
use crate::messages::ContextMessage;

mod crypto;
pub mod external;

#[derive(Clone, Debug)]
pub struct ContextClient {
    datastore: Store,
    external_client: ExternalClient<AnyTransport>,
    context_manager: LazyRecipient<ContextMessage>,
}

impl ContextClient {
    pub fn has_context(&self, context_id: &ContextId) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        Ok(handle.has(&key)?)
    }

    pub fn get_context(&self, context_id: &ContextId) -> eyre::Result<Option<Context>> {
        let handle = self.datastore.handle();

        let key = key::ContextMeta::new(*context_id);

        let Some(context) = handle.get(&key)? else {
            return Ok(None);
        };

        let context = Context::new(
            *context_id,
            context.application.application_id(),
            context.root_hash.into(),
        );

        Ok(Some(context))
    }

    pub async fn get_contexts(
        &self,
        start: Option<ContextId>,
    ) -> impl Stream<Item = eyre::Result<ContextId>> {
        let datastore = self.datastore.clone();

        try_stream! {
            let handle = datastore.handle();
            let iter_result = handle.iter::<ContextMetaKey>();
            let mut iter = iter_result?;

            let start = start.and_then(|s| iter.seek(ContextMetaKey::new(s)).transpose());

            for key in start.into_iter().chain(iter.keys()) {
                yield key?.context_id();
            }
        }
    }

    pub async fn delete_context(&self, context_id: &ContextId) -> eyre::Result<bool> {
        let mut handle = self.datastore.handle();

        let key = ContextMetaKey::new(*context_id);
        if !handle.has(&key)? {
            return Ok(false);
        }

        handle.delete(&key)?;

        handle.delete(&ContextConfigKey::new(*context_id))?;

        let identity_keys = {
            let mut iter = handle.iter::<ContextIdentityKey>()?;
            iter.keys()
                .filter_map(Result::ok)
                .filter(|k| k.context_id() == *context_id)
                .collect::<Vec<_>>()
        };

        for key in identity_keys {
            handle.delete(&key)?;
        }

        Ok(true)
    }

    pub async fn context_members(
        &self,
        context_id: &ContextId,
        only_owned: Option<bool>,
    ) -> impl Stream<Item = eyre::Result<PublicKey>> {
        let datastore = self.datastore.clone();
        let context_id = *context_id;
        let only_owned = only_owned.unwrap_or(false);

        try_stream! {
            let handle = datastore.handle();
            let mut iter = handle.iter::<ContextIdentityKey>()?;

            let first = iter
                .seek(ContextIdentityKey::new(context_id, [0; 32].into()))
                .transpose()
                .map(|k| (k, iter.read()));

            for (k, v) in first.into_iter().chain(iter.entries()) {
                let (k, v) = (k?, v?);

                if k.context_id() != context_id {
                    break;
                }

                if !only_owned || v.private_key.is_some() {
                    yield k.public_key();
                }
            }
        }
    }

    pub async fn has_member(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
    ) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = ContextIdentityKey::new(*context_id, *public_key);

        Ok(handle.has(&key)?)
    }

    pub async fn update_application(
        &self,
        context_id: &ContextId,
        application_id: &ApplicationId,
        identity: &PublicKey,
    ) -> eyre::Result<()> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::UpdateApplication {
                request: UpdateApplicationRequest {
                    context_id: *context_id,
                    application_id: *application_id,
                    public_key: *identity,
                },
                outcome: sender,
            })
            .await
            .expect("Context manager mailbox not to be dropped");

        receiver
            .await
            .expect("Context manager not to drop response channel")
    }

    pub async fn invite_member(
        &self,
        context_id: &ContextId,
        inviter_id: &PublicKey,
        invitee_id: &PublicKey,
    ) -> eyre::Result<Option<ContextInvitationPayload>> {
        let handle = self.datastore.handle();

        let Some(context_config) = handle.get(&ContextConfigKey::new(*context_id))? else {
            return Ok(None);
        };

        let Some(ContextIdentityValue {
            private_key: Some(requester_secret),
            ..
        }) = handle.get(&ContextIdentityKey::new(*context_id, *inviter_id))?
        else {
            return Ok(None);
        };

        let nonce = self
            .external_client
            .query::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .fetch_nonce(
                context_id.rt().expect("infallible conversion"),
                inviter_id.rt().expect("infallible conversion"),
            )
            .await?
            .ok_or_eyre("The inviter doesen't exist")?;

        self.external_client
            .mutate::<ContextConfigEnv>(
                context_config.protocol.as_ref().into(),
                context_config.network.as_ref().into(),
                context_config.contract.as_ref().into(),
            )
            .add_members(
                context_id.rt().expect("infallible conversion"),
                &[invitee_id.rt().expect("infallible conversion")],
            )
            .send(requester_secret, nonce)
            .await?;

        let invitation_payload = ContextInvitationPayload::new(
            *context_id,
            *invitee_id,
            context_config.protocol.into_string().into(),
            context_config.network.into_string().into(),
            context_config.contract.into_string().into(),
        )?;

        Ok(Some(invitation_payload))
    }

    pub async fn create_context(
        &self,
        protocol: String,
        application_id: &ApplicationId,
        identity_secret: Option<PrivateKey>,
        init_params: Vec<u8>,
        seed: Option<[u8; 32]>,
    ) -> eyre::Result<CreateContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::CreateContext {
                request: CreateContextRequest {
                    protocol,
                    seed,
                    application_id: *application_id,
                    identity_secret,
                    init_params,
                },
                outcome: sender,
            })
            .await
            .expect("Context manager mailbox not to be dropped");

        receiver
            .await
            .expect("Context manager not to drop response channel")
    }

    pub async fn execute(
        &self,
        context: &ContextId,
        method: String,
        payload: Vec<u8>,
        executor: &PublicKey,
    ) -> Result<ExecuteResponse, ExecuteError> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::Execute {
                request: ExecuteRequest {
                    context: *context,
                    method,
                    payload,
                    executor: *executor,
                },
                outcome: sender,
            })
            .await
            .expect("Context manager mailbox not to be dropped");

        receiver
            .await
            .expect("Context manager not to drop response channel")
    }
}
