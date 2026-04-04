use calimero_server_primitives::admin::{
    CreateGroupInvitationApiRequest, CreateNamespaceApiRequest, CreateNamespaceApiResponse,
    DeleteNamespaceApiRequest, DeleteNamespaceApiResponse, JoinGroupApiRequest, JoinGroupApiResponse,
    ListNamespaceGroupsApiResponse, ListNamespacesApiResponse, NamespaceApiResponse,
    NamespaceIdentityApiResponse,
};
use eyre::Result;

use super::{ClientAuthenticator, ClientStorage};

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
        let response = self.connection.post("admin-api/namespaces", request).await?;
        Ok(response)
    }

    pub async fn get_namespace(&self, namespace_id: &str) -> Result<NamespaceApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/namespaces/{namespace_id}"))
            .await?;
        Ok(response)
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

    pub async fn create_namespace_invitation(
        &self,
        namespace_id: &str,
        request: CreateGroupInvitationApiRequest,
    ) -> Result<serde_json::Value> {
        let response = self
            .connection
            .post(&format!("admin-api/namespaces/{namespace_id}/invite"), request)
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
            .post(&format!("admin-api/namespaces/{namespace_id}/join"), request)
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
}
