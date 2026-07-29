use calimero_server_primitives::admin::{
    CreateAccountApiRequest, CreateAccountApiResponse, CreateGroupInvitationApiRequest,
    CreateNamespaceApiRequest, CreateNamespaceApiResponse, DeleteNamespaceApiRequest,
    DeleteNamespaceApiResponse, GetNamespaceApiResponse, JoinGroupApiRequest, JoinGroupApiResponse,
    ListNamespaceGroupsApiResponse, ListNamespacesApiResponse, NamespaceApiResponse,
    NamespaceIdentityApiResponse, PairDeviceCompleteApiRequest, PairDeviceCompleteApiResponse,
    PairDeviceInitApiRequest, PairDeviceInitApiResponse, RevokeDeviceApiRequest,
    RevokeDeviceApiResponse,
};
use eyre::Result;
use serde::Serialize;

use super::{ClientAuthenticator, ClientStorage};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateGroupInNamespaceApiRequest {
    group_name: Option<String>,
}

impl<A, S> super::Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn list_namespaces(&self) -> Result<ListNamespacesApiResponse> {
        let response = self.connection.get("admin-api/namespaces").await?;
        Ok(response)
    }

    pub async fn get_namespace_identity(
        &self,
        namespace_id: &str,
    ) -> Result<NamespaceIdentityApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/namespaces/{namespace_id}/identity"))
            .await?;
        Ok(response)
    }

    pub async fn list_namespaces_for_application(
        &self,
        application_id: &str,
    ) -> Result<ListNamespacesApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/namespaces/for-application/{application_id}"
            ))
            .await?;
        Ok(response)
    }

    pub async fn create_namespace(
        &self,
        request: CreateNamespaceApiRequest,
    ) -> Result<CreateNamespaceApiResponse> {
        let response = self
            .connection
            .post("admin-api/namespaces", request)
            .await?;
        Ok(response)
    }

    pub async fn get_namespace(&self, namespace_id: &str) -> Result<NamespaceApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/namespaces/{namespace_id}"))
            .await?;
        let response: GetNamespaceApiResponse = response;
        Ok(response.data)
    }

    pub async fn delete_namespace(
        &self,
        namespace_id: &str,
        request: DeleteNamespaceApiRequest,
    ) -> Result<DeleteNamespaceApiResponse> {
        let response = self
            .connection
            .delete_with_body(&format!("admin-api/namespaces/{namespace_id}"), request)
            .await?;
        Ok(response)
    }

    /// Enroll this node's device into a namespace under a fresh account.
    ///
    /// Must follow key delivery: the device link travels as an encrypted group
    /// op, so a node holding no scope key cannot publish one. The node refuses
    /// with that reason rather than failing obscurely.
    pub async fn create_account(&self, namespace_id: &str) -> Result<CreateAccountApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/account"),
                CreateAccountApiRequest {},
            )
            .await?;
        Ok(response)
    }

    /// Mint a device on this node for an account that already exists elsewhere
    /// — the first half of pairing.
    ///
    /// Needs no scope key, unlike [`Self::create_account`]: nothing is
    /// published here. It returns the device id and agreement key that the
    /// account holder certifies in the second half.
    pub async fn pair_device_init(
        &self,
        namespace_id: &str,
        request: PairDeviceInitApiRequest,
    ) -> Result<PairDeviceInitApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/account/pair-init"),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Certify a device another node minted, link it, and deliver the scope
    /// key — the second half of pairing.
    ///
    /// Run on the node that holds the account. Needs the current scope key:
    /// the link is an encrypted group op, and the delivery is that same key
    /// wrapped for the new device.
    pub async fn pair_device_complete(
        &self,
        namespace_id: &str,
        request: PairDeviceCompleteApiRequest,
    ) -> Result<PairDeviceCompleteApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/account/pair-complete"),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Withdraw a device from an account, terminally.
    ///
    /// An admin may revoke any device and rotates the scope key in the same op.
    /// The account holder may revoke its own with a root-signed proof, but
    /// cannot rotate — so the device stops writing at once and keeps reading
    /// until an admin rotates.
    pub async fn revoke_device(
        &self,
        namespace_id: &str,
        request: RevokeDeviceApiRequest,
    ) -> Result<RevokeDeviceApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/account/revoke"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn create_namespace_invitation(
        &self,
        namespace_id: &str,
        request: CreateGroupInvitationApiRequest,
    ) -> Result<serde_json::Value> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/invite"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn join_namespace(
        &self,
        namespace_id: &str,
        request: JoinGroupApiRequest,
    ) -> Result<JoinGroupApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/join"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn list_namespace_groups(
        &self,
        namespace_id: &str,
    ) -> Result<ListNamespaceGroupsApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/namespaces/{namespace_id}/groups"))
            .await?;
        Ok(response)
    }

    pub async fn create_group_in_namespace(
        &self,
        namespace_id: &str,
        group_name: Option<String>,
    ) -> Result<serde_json::Value> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/groups"),
                CreateGroupInNamespaceApiRequest { group_name },
            )
            .await?;
        Ok(response)
    }
}
