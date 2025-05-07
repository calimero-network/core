use async_stream::try_stream;
use calimero_context_config::client::{AnyTransport, Client as ExternalClient};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::context::{Context, ContextId};
use calimero_primitives::identity::PublicKey;
use calimero_store::{key, Store};
use calimero_utils_actix::LazyRecipient;
use futures_util::{stream, Stream};

use crate::messages::create_context::{CreateContextRequest, CreateContextResponse};
use crate::messages::delete_context::{DeleteContextRequest, DeleteContextResponse};
use crate::messages::execute::{ExecuteError, ExecuteRequest, ExecuteResponse};
use crate::messages::join_context::{JoinContextRequest, JoinContextResponse};
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

    pub async fn get_contexts(&self) -> impl Stream<Item = eyre::Result<ContextId>> {
        stream::empty()
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
            .expect("Context manager mailbox not to be dropped");

        receiver
            .await
            .expect("Context manager not to drop response channel")
    }

    pub async fn context_members(
        &self,
        context_id: &ContextId,
        only_owned: Option<bool>,
    ) -> impl Stream<Item = eyre::Result<PublicKey>> {
        stream::empty()
    }

    pub async fn has_member(
        &self,
        context_id: &ContextId,
        public_key: &PublicKey,
        is_present: bool,
    ) -> eyre::Result<bool> {
        todo!()
    }

    pub async fn update_application_id(
        &self,
        context_id: &ContextId,
        application_id: &ApplicationId,
        identity: &PublicKey,
    ) -> eyre::Result<()> {
        todo!()
    }

    pub async fn join_context(
        &self,
        identity_secret: PrivateKey,
        invitation_payload: ContextInvitationPayload,
    ) -> eyre::Result<JoinContextResponse> {
        let (sender, receiver) = oneshot::channel();

        self.context_manager
            .send(ContextMessage::JoinContext {
                request: JoinContextRequest {
                    identity_secret,
                    invitation_payload,
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
