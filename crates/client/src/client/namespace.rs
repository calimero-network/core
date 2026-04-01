use calimero_server_primitives::admin::{
    ListNamespacesApiResponse, NamespaceIdentityApiResponse,
};

use crate::client::Client;
use crate::ClientError;

type Result<T> = std::result::Result<T, ClientError>;

impl Client {
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
}
