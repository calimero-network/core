//! Application management API methods.

use calimero_primitives::application::ApplicationId;
use calimero_server_primitives::admin::{
    GetApplicationResponse, InstallApplicationRequest, InstallApplicationResponse,
    InstallDevApplicationRequest, ListApplicationsResponse, UninstallApplicationResponse,
};
use eyre::Result;

use super::Client;
use crate::traits::{ClientAuthenticator, ClientStorage};

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn get_application(&self, app_id: &ApplicationId) -> Result<GetApplicationResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/applications/{app_id}"))
            .await?;
        Ok(response)
    }

    pub async fn install_dev_application(
        &self,
        request: InstallDevApplicationRequest,
    ) -> Result<InstallApplicationResponse> {
        let response = self
            .connection
            .post("admin-api/install-dev-application", request)
            .await?;
        Ok(response)
    }

    pub async fn install_application(
        &self,
        request: InstallApplicationRequest,
    ) -> Result<InstallApplicationResponse> {
        let response = self
            .connection
            .post("admin-api/install-application", request)
            .await?;
        Ok(response)
    }

    pub async fn list_applications(&self) -> Result<ListApplicationsResponse> {
        let response = self.connection.get("admin-api/applications").await?;
        Ok(response)
    }

    pub async fn uninstall_application(
        &self,
        app_id: &ApplicationId,
    ) -> Result<UninstallApplicationResponse> {
        let response = self
            .connection
            .delete(&format!("admin-api/applications/{app_id}"))
            .await?;
        Ok(response)
    }
}
