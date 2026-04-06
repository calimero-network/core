//! Context and identity API operations for the Calimero client.

use calimero_primitives::context::ContextId;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    CreateContextRequest, CreateContextResponse, DeleteContextApiRequest, DeleteContextResponse,
    GetContextClientKeysResponse, GetContextIdentitiesResponse, GetContextResponse,
    GetContextStorageResponse, GetContextsResponse, SyncContextResponse,
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

    pub async fn delete_context(
        &self,
        context_id: &ContextId,
        requester: Option<PublicKey>,
    ) -> Result<DeleteContextResponse> {
        let response = self
            .connection
            .delete_with_body(
                &format!("admin-api/contexts/{context_id}"),
                DeleteContextApiRequest { requester },
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

    pub async fn get_context_client_keys(
        &self,
        context_id: &ContextId,
    ) -> Result<GetContextClientKeysResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/contexts/{context_id}/client-keys"))
            .await?;
        Ok(response)
    }

    pub async fn sync_context(&self, context_id: &ContextId) -> Result<SyncContextResponse> {
        let response = self
            .connection
            .post_no_body(&format!("admin-api/contexts/sync/{context_id}"))
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
