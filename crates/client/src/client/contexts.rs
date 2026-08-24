//! Context and identity API operations for the Calimero client.

use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponse, DeleteContextApiRequest, DeleteContextResponse,
    GetContextIdentitiesResponse, GetContextResponse, GetContextStorageResponse,
    GetContextsResponse, PerformIntentApiRequest, PerformIntentApiResponse,
    ResyncContextApiRequest, ResyncContextApiResponse, SyncContextResponse,
    UpdateContextApplicationRequest, UpdateContextApplicationResponse,
};
use eyre::Result;

use super::Client;
use crate::traits::{ClientAuthenticator, ClientStorage};

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn update_context_application(
        &self,
        context_id: &ContextId,
        request: UpdateContextApplicationRequest,
    ) -> Result<UpdateContextApplicationResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/contexts/{context_id}/application"),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Ask this node to run one method on a member's behalf, under a warrant
    /// that member signed.
    ///
    /// The caller supplies only its own half — the warrant and the proof its
    /// signing key is a device of the account it names. The node attaches its own
    /// credential, so a client never has to learn which of the node's processes
    /// runs the intent.
    pub async fn perform_intent(
        &self,
        context_id: &str,
        request: PerformIntentApiRequest,
    ) -> Result<PerformIntentApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/contexts/{context_id}/intents"), request)
            .await?;
        Ok(response)
    }

    pub async fn get_context(&self, context_id: &ContextId) -> Result<GetContextResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/contexts/{context_id}"))
            .await?;
        Ok(response)
    }

    pub async fn list_contexts(&self) -> Result<GetContextsResponse> {
        let response = self.connection.get("admin-api/contexts").await?;
        Ok(response)
    }

    pub async fn create_context(
        &self,
        request: CreateContextRequest,
    ) -> Result<CreateContextResponse> {
        let response = self.connection.post("admin-api/contexts", request).await?;
        Ok(response)
    }

    pub async fn delete_context(&self, context_id: &ContextId) -> Result<DeleteContextResponse> {
        let response = self
            .connection
            .delete_with_body(
                &format!("admin-api/contexts/{context_id}"),
                DeleteContextApiRequest {},
            )
            .await?;
        Ok(response)
    }

    pub async fn get_context_storage(
        &self,
        context_id: &ContextId,
    ) -> Result<GetContextStorageResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/contexts/{context_id}/storage"))
            .await?;
        Ok(response)
    }

    pub async fn get_context_identities(
        &self,
        context_id: &ContextId,
        owned: bool,
    ) -> Result<GetContextIdentitiesResponse> {
        let endpoint = if owned {
            format!("admin-api/contexts/{context_id}/identities-owned")
        } else {
            format!("admin-api/contexts/{context_id}/identities")
        };

        let response = self.connection.get(&endpoint).await?;
        Ok(response)
    }

    pub async fn sync_context(&self, context_id: &ContextId) -> Result<SyncContextResponse> {
        let response = self
            .connection
            .post_no_body(&format!("admin-api/contexts/sync/{context_id}"))
            .await?;
        Ok(response)
    }

    /// Resync a stranded context by adopting a peer's full state. Destructive:
    /// `force` must be set when the context still holds local DAG heads, which
    /// the resync discards.
    pub async fn resync_context(
        &self,
        context_id: &str,
        request: ResyncContextApiRequest,
    ) -> Result<ResyncContextApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/contexts/{context_id}/resync"), request)
            .await?;
        Ok(response)
    }

    /// Sync all contexts (legacy method for backward compatibility)
    pub async fn sync_all_contexts(&self) -> Result<SyncContextResponse> {
        let response = self
            .connection
            .post_no_body("admin-api/contexts/sync")
            .await?;
        Ok(response)
    }
}
