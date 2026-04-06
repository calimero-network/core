//! System-level operations for the Calimero client.

use calimero_server_primitives::admin::{
    GenerateContextIdentityResponse, GetPeersCountResponse, InviteSpecializedNodeRequest,
    InviteSpecializedNodeResponse,
};
use eyre::Result;

use super::Client;
use crate::traits::{ClientAuthenticator, ClientStorage};

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn generate_context_identity(&self) -> Result<GenerateContextIdentityResponse> {
        let response = self
            .connection
            .post("admin-api/identity/context", ())
            .await?;
        Ok(response)
    }

    pub async fn get_peers_count(&self) -> Result<GetPeersCountResponse> {
        let response = self.connection.get("admin-api/peers").await?;
        Ok(response)
    }

    /// Invite specialized nodes (e.g., read-only TEE nodes) to join a context.
    ///
    /// This broadcasts a specialized node discovery request to the global invite topic.
    /// Specialized nodes listening will respond with verification and receive invitations.
    pub async fn invite_specialized_node(
        &self,
        request: InviteSpecializedNodeRequest,
    ) -> Result<InviteSpecializedNodeResponse> {
        let response = self
            .connection
            .post("admin-api/contexts/invite-specialized-node", request)
            .await?;
        Ok(response)
    }
}
