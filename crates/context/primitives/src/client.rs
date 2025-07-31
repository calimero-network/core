#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use std::num::NonZeroUsize;

use async_stream::try_stream;
use calimero_context_config::client::{AnyTransport, Client as ExternalClient};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, types, Store};
use calimero_utils_actix::LazyRecipient;
use eyre::{bail, eyre};
use futures_util::Stream;
use tokio::sync::oneshot;

use crate::messages::create_context::{CreateContextRequest, CreateContextResponse};
use crate::messages::delete_context::{DeleteContextRequest, DeleteContextResponse};
use crate::messages::execute::{ExecuteError, ExecuteRequest, ExecuteResponse};
use crate::messages::join_context::{JoinContextRequest, JoinContextResponse};
use crate::messages::update_application::UpdateApplicationRequest;
use crate::messages::ContextMessage;
use crate::ContextAtomic;

pub mod crypto;
pub mod external;
mod sync;

#[derive(Clone, Debug)]
pub struct ContextClient {
    datastore: Store,
    node_client: NodeClient,
    external_client: ExternalClient<AnyTransport>,
    context_manager: LazyRecipient<ContextMessage>,
}

impl ContextClient {
    pub fn new(
        datastore: Store,
        node_client: NodeClient,
        external_client: ExternalClient<AnyTransport>,
        context_manager: LazyRecipient<ContextMessage>,
    ) -> Self {
        Self {
            datastore,
            node_client,
            external_client,
            context_manager,
        }
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
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    pub async fn invite_member(
        &self,
        context_id: &ContextId,
        inviter_id: &PublicKey,
        invitee_id: &PublicKey,
    ) -> eyre::Result<Option<ContextInvitationPayload>> {
        let Some(external_config) = self.context_config(context_id)? else {
            return Ok(None);
        };

        let external_client = self.external_client(context_id, &external_config)?;

        external_client
            .config()
            .add_members(inviter_id, &[*invitee_id])
            .await?;

        let invitation_payload = ContextInvitationPayload::new(
            *context_id,
            *invitee_id,
            external_config.protocol,
            external_config.network_id,
            external_config.contract_id,
        )?;

        Ok(Some(invitation_payload))
    }

    pub async fn join_context(
        &self,
        invitation_payload: ContextInvitationPayload,
    ) -> eyre::Result<JoinContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::JoinContext {
                request: JoinContextRequest { invitation_payload },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

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

    pub fn get_contexts(
        &self,
        start: Option<ContextId>,
    ) -> impl Stream<Item = eyre::Result<ContextId>> {
        let handle = self.datastore.handle();

        try_stream! {
            let mut iter = handle.iter::<key::ContextMeta>()?;

            let start = start.and_then(|s| iter.seek(key::ContextMeta::new(s)).transpose());

            for key in start.into_iter().chain(iter.keys()) {
                yield key?.context_id();
            }
        }
    }

    pub fn has_member(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
        // is_owned: Option<bool>,
    ) -> eyre::Result<bool> {
        let handle = self.datastore.handle();

        let key = key::ContextIdentity::new(*context_id, *public_key);

        Ok(handle.has(&key)?)
    }

    pub fn context_members(
        &self,
        context_id: &ContextId,
        owned: Option<bool>,
    ) -> impl Stream<Item = eyre::Result<(PublicKey, bool)>> {
        let handle = self.datastore.handle();
        let context_id = *context_id;
        let only_owned = owned.unwrap_or(false);

        try_stream! {
            let mut iter = handle.iter::<key::ContextIdentity>()?;

            let first = iter
                .seek(key::ContextIdentity::new(context_id, [0; 32].into()))
                .transpose()
                .map(|k| (k, iter.read()));

            for (k, v) in first.into_iter().chain(iter.entries()) {
                let (k, v) = (k?, v?);

                if k.context_id() != context_id {
                    break;
                }

                let is_owned = v.private_key.is_some();
                if !only_owned || is_owned {
                    yield (k.public_key(), is_owned);
                }
            }
        }
    }

    pub async fn execute(
        &self,
        context: &ContextId,
        executor: &PublicKey,
        method: String,
        payload: Vec<u8>,
        aliases: Vec<Alias<PublicKey>>,
        atomic: Option<ContextAtomic>,
    ) -> Result<ExecuteResponse, ExecuteError> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::Execute {
                request: ExecuteRequest {
                    context: *context,
                    executor: *executor,
                    method,
                    payload,
                    aliases,
                    atomic,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
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
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    pub async fn delete_context(
        &self,
        context_id: &ContextId,
    ) -> eyre::Result<DeleteContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::DeleteContext {
                request: DeleteContextRequest {
                    context_id: *context_id,
                },
                outcome: sender,
            })
            .await
            .expect("Mailbox not to be dropped");

        receiver.await.expect("Mailbox not to be dropped")
    }

    pub fn set_delta_height(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
        height: NonZeroUsize,
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        handle.put(
            &key::ContextDelta::new(*context_id, *public_key, 0),
            &types::ContextDelta::Head(height),
        )?;

        Ok(())
    }

    pub fn get_delta_height(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
    ) -> eyre::Result<Option<NonZeroUsize>> {
        let handle = self.datastore.handle();

        let key = key::ContextDelta::new(*context_id, *public_key, 0);

        let Some(delta) = handle.get(&key)? else {
            return Ok(None);
        };

        let types::ContextDelta::Head(height) = delta else {
            bail!("Odd HEAD delta format for context: {context_id}, public key: {public_key}");
        };

        Ok(Some(height))
    }

    pub fn put_state_delta(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
        height: &NonZeroUsize,
        delta: &[u8],
    ) -> eyre::Result<()> {
        let mut handle = self.datastore.handle();

        handle.put(
            &key::ContextDelta::new(*context_id, *public_key, height.get()),
            &types::ContextDelta::Data(delta.into()),
        )?;

        Ok(())
    }

    pub fn get_state_deltas(
        &self,
        context_id: &ContextId,
        public_key: Option<&PublicKey>,
        start_height: NonZeroUsize,
    ) -> impl Stream<Item = eyre::Result<(PublicKey, NonZeroUsize, Box<[u8]>)>> {
        let handle = self.datastore.handle();
        let context_id = *context_id;
        let public_key = public_key.copied();

        try_stream! {
            let mut iter = handle.iter::<key::ContextDelta>()?;

            let first = iter
                .seek(key::ContextDelta::new(
                    context_id,
                    public_key.unwrap_or([0; 32].into()),
                    start_height.get(),
                ))
                .transpose()
                .map(|k| (k, iter.read()));

            for (k, v) in first.into_iter().chain(iter.entries()) {
                let (k, v) = (k?, v?);

                if k.context_id() != context_id {
                    break;
                }

                let expected = k.public_key();
                if let Some(public_key) = public_key {
                    if expected != public_key {
                        break;
                    }
                }

                let Some(height) = NonZeroUsize::new(k.height()) else {
                    continue;
                };

                let types::ContextDelta::Data(delta) = v else {
                    return Err(eyre!("Odd DATA delta format for context: {context_id}, public key: {expected}, height: {height}"))?;
                };

                yield (expected, height, delta.into());
            }
        }
    }
}
