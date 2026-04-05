//! Package API methods for the Calimero client.

use calimero_server_primitives::admin::{
    GetLatestVersionResponse, ListPackagesResponse, ListVersionsResponse,
};
use eyre::Result;

use super::Client;
use crate::traits::{ClientAuthenticator, ClientStorage};

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn list_packages(&self) -> Result<ListPackagesResponse> {
        let response = self.connection.get("admin-api/packages").await?;
        Ok(response)
    }

    pub async fn list_versions(&self, package: &str) -> Result<ListVersionsResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/packages/{package}/versions"))
            .await?;
        Ok(response)
    }

    pub async fn get_latest_version(&self, package: &str) -> Result<GetLatestVersionResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/packages/{package}/latest"))
            .await?;
        Ok(response)
    }
}
