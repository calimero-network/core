#![allow(clippy::multiple_inherent_impl, reason = "better readability")]

use async_stream::try_stream;
use calimero_context_config::client::{AnyTransport, Client as ExternalClient};
use calimero_node_primitives::client::NodeClient;
use calimero_primitives::alias::Alias;
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId, ContextInvitationPayload};
use calimero_primitives::identity::{PrivateKey, PublicKey};
use calimero_store::{key, Store};
use calimero_utils_actix::LazyRecipient;
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
        let datastore = self.datastore.handle();

        try_stream! {
            let mut iter = datastore.iter::<key::ContextMeta>()?;

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
        let datastore = self.datastore.handle();
        let context_id = *context_id;
        let only_owned = owned.unwrap_or(false);

        try_stream! {
            let mut iter = datastore.iter::<key::ContextIdentity>()?;

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
}
