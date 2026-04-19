//! System-level operations for the Calimero client.

use calimero_server_primitives::admin::{
    FleetJoinRequest, FleetJoinResponse, GenerateContextIdentityResponse, GetPeersCountResponse,
    InviteSpecializedNodeRequest, InviteSpecializedNodeResponse,
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

    /// Announce this node as a TEE fleet member for the given group.
    ///
    /// Calls POST /admin-api/tee/fleet-join. The local node generates a TDX
    /// attestation, broadcasts `TeeAttestationAnnounce` on the namespace
    /// topic, waits for admission (up to 30s), and auto-joins all contexts
    /// in the group. Used by the mero-tee fleet sidecar after the manager
    /// assigns it to a group via /api/fleet/should-join.
    pub async fn fleet_join(&self, group_id: String) -> Result<FleetJoinResponse> {
        let response = self
            .connection
            .post("admin-api/tee/fleet-join", FleetJoinRequest { group_id })
            .await?;
        Ok(response)
    }
}
