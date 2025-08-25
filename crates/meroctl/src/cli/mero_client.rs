use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    DeleteContextResponse, GenerateContextIdentityResponse, GetApplicationResponse, GetContextClientKeysResponse,
    GetContextIdentitiesResponse, GetContextResponse, GetContextStorageResponse, GetContextsResponse,
    GetPeersCountResponse, GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
    GrantPermissionResponse, InstallApplicationRequest, InstallApplicationResponse,
    InstallDevApplicationRequest, InviteToContextRequest, InviteToContextResponse,
    JoinContextRequest, JoinContextResponse, ListApplicationsResponse, RevokePermissionResponse,
    SyncContextResponse, UninstallApplicationResponse, UpdateContextApplicationRequest,
    UpdateContextApplicationResponse,
};
use calimero_server_primitives::jsonrpc::{Request, Response};
use eyre::Result;
use serde::{Deserialize, Serialize};

use crate::output::Report;

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct BlobDeleteResponse {
    pub blob_id: BlobId,
    pub deleted: bool,
}

impl Report for BlobDeleteResponse {
    fn report(&self) {
        if self.deleted {
            println!("Successfully deleted blob '{}'", self.blob_id);
        } else {
            println!(
                "Failed to delete blob '{}' (blob may not exist)",
                self.blob_id
            );
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponse {
    pub data: BlobListResponseData,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponseData {
    pub blobs: Vec<BlobInfo>,
}

impl Report for BlobListResponse {
    fn report(&self) {
        if self.data.blobs.is_empty() {
            println!("No blobs found");
        } else {
            let mut table = comfy_table::Table::new();
            let _ = table.set_header(vec![
                comfy_table::Cell::new("Blob ID").fg(comfy_table::Color::Blue),
                comfy_table::Cell::new("Size").fg(comfy_table::Color::Blue),
            ]);
            for blob in &self.data.blobs {
                let _ = table.add_row(vec![
                    blob.blob_id.to_string(),
                    format!("{} bytes", blob.size),
                ]);
            }
            println!("{table}");
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobInfoResponse {
    pub data: BlobMetadata,
}

impl Report for BlobInfoResponse {
    fn report(&self) {
        let mut table = comfy_table::Table::new();
        let _ = table.set_header(vec![
            comfy_table::Cell::new("Blob ID").fg(comfy_table::Color::Blue),
            comfy_table::Cell::new("Size (bytes)").fg(comfy_table::Color::Blue),
            comfy_table::Cell::new("MIME Type").fg(comfy_table::Color::Blue),
            comfy_table::Cell::new("Hash").fg(comfy_table::Color::Blue),
        ]);

        let _ = table.add_row(vec![
            &self.data.blob_id.to_string(),
            &self.data.size.to_string(),
            &self.data.mime_type,
            &hex::encode(self.data.hash),
        ]);

        println!("{table}");
    }
}

#[derive(Clone, Debug)]
pub struct MeroClient {
    base_url: String,
    http_client: reqwest::Client,
}

impl MeroClient {
    pub fn new(base_url: String) -> Self {
        Self {
            base_url,
            http_client: reqwest::Client::new(),
        }
    }

    pub async fn get_application(&self, app_id: &ApplicationId) -> Result<GetApplicationResponse> {
        let url = format!("{}/admin-api/applications/{}", self.base_url, app_id);

        let response = self.http_client.get(&url).send().await?;
        let application_response: GetApplicationResponse = response.json().await?;

        Ok(application_response)
    }

    pub async fn install_dev_application(
        &self,
        request: InstallDevApplicationRequest,
    ) -> Result<InstallApplicationResponse> {
        let url = format!("{}/admin-api/install-dev-application", self.base_url);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let install_response: InstallApplicationResponse = response.json().await?;

        Ok(install_response)
    }


    pub async fn install_application(
        &self,
        request: InstallApplicationRequest,
    ) -> Result<InstallApplicationResponse> {
        let url = format!("{}/admin-api/install-application", self.base_url);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let install_response: InstallApplicationResponse = response.json().await?;

        Ok(install_response)
    }

    pub async fn list_applications(&self) -> Result<ListApplicationsResponse> {
        let url = format!("{}/admin-api/applications", self.base_url);

        let response = self.http_client.get(&url).send().await?;
        let applications_response: ListApplicationsResponse = response.json().await?;

        Ok(applications_response)
    }

    pub async fn uninstall_application(
        &self,
        app_id: &ApplicationId,
    ) -> Result<UninstallApplicationResponse> {
        let url = format!("{}/admin-api/applications/{}", self.base_url, app_id);

        let response = self.http_client.delete(&url).send().await?;
        let uninstall_response: UninstallApplicationResponse = response.json().await?;

        Ok(uninstall_response)
    }

    pub async fn delete_blob(&self, blob_id: &BlobId) -> Result<BlobDeleteResponse> {
        let url = format!("{}/admin-api/blobs/{}", self.base_url, blob_id);

        let response = self.http_client.delete(&url).send().await?;
        let delete_response: BlobDeleteResponse = response.json().await?;

        Ok(delete_response)
    }

    pub async fn list_blobs(&self) -> Result<BlobListResponse> {
        let url = format!("{}/admin-api/blobs", self.base_url);

        let response = self.http_client.get(&url).send().await?;
        let blobs_response: BlobListResponse = response.json().await?;

        Ok(blobs_response)
    }

    pub async fn get_blob_info(&self, blob_id: &BlobId) -> Result<BlobInfoResponse> {
        let url = format!("{}/admin-api/blobs/{}", self.base_url, blob_id);

        let response = self.http_client.head(&url).send().await?;
        let headers = response.headers();

        let size = headers
            .get("content-length")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let mime_type = headers
            .get("content-type")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();

        let hash_hex = headers
            .get("x-blob-hash")
            .and_then(|h| h.to_str().ok())
            .unwrap_or("");

        let hash =
            hex::decode(hash_hex).map_err(|_| eyre::eyre!("Invalid hash in response headers"))?;

        let hash_array: [u8; 32] = hash
            .try_into()
            .map_err(|_| eyre::eyre!("Hash must be 32 bytes"))?;

        let blob_info = BlobInfoResponse {
            data: BlobMetadata {
                blob_id: *blob_id,
                size,
                mime_type,
                hash: hash_array,
            },
        };

        Ok(blob_info)
    }

    pub async fn generate_context_identity(&self) -> Result<GenerateContextIdentityResponse> {
        let url = format!("{}/admin-api/identity/context", self.base_url);

        let response = self.http_client.post(&url).send().await?;
        let identity_response: GenerateContextIdentityResponse = response.json().await?;

        Ok(identity_response)
    }

    pub async fn get_peers_count(&self) -> Result<GetPeersCountResponse> {
        let url = format!("{}/admin-api/peers", self.base_url);

        let response = self.http_client.get(&url).send().await?;
        let peers_response: GetPeersCountResponse = response.json().await?;

        Ok(peers_response)
    }

    pub async fn execute_jsonrpc<P>(&self, request: Request<P>) -> Result<Response>
    where
        P: Serialize,
    {
        let url = format!("{}/jsonrpc", self.base_url);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let jsonrpc_response: Response = response.json().await?;

        Ok(jsonrpc_response)
    }

    pub async fn grant_permissions(
        &self,
        context_id: &ContextId,
        request: Vec<(PublicKey, calimero_context_config::types::Capability)>,
    ) -> Result<GrantPermissionResponse> {
        let url = format!("{}/admin-api/contexts/{}/capabilities/grant", self.base_url, context_id);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let grant_response: GrantPermissionResponse = response.json().await?;

        Ok(grant_response)
    }

    pub async fn revoke_permissions(
        &self,
        context_id: &ContextId,
        request: Vec<(PublicKey, calimero_context_config::types::Capability)>,
    ) -> Result<RevokePermissionResponse> {
        let url = format!("{}/admin-api/contexts/{}/capabilities/revoke", self.base_url, context_id);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let revoke_response: RevokePermissionResponse = response.json().await?;

        Ok(revoke_response)
    }

    pub async fn invite_to_context(
        &self,
        request: InviteToContextRequest,
    ) -> Result<InviteToContextResponse> {
        let url = format!("{}/admin-api/contexts/invite", self.base_url);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let invite_response: InviteToContextResponse = response.json().await?;

        Ok(invite_response)
    }

    pub async fn update_context_application(
        &self,
        context_id: &ContextId,
        request: UpdateContextApplicationRequest,
    ) -> Result<UpdateContextApplicationResponse> {
        let url = format!("{}/admin-api/contexts/{}/application", self.base_url, context_id);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let update_response: UpdateContextApplicationResponse = response.json().await?;

        Ok(update_response)
    }

    pub async fn get_proposal(
        &self,
        context_id: &ContextId,
        proposal_id: &Hash,
    ) -> Result<GetProposalResponse> {
        let url = format!("{}/admin-api/contexts/{}/proposals/{}", self.base_url, context_id, proposal_id);

        let response = self.http_client.get(&url).send().await?;
        let proposal_response: GetProposalResponse = response.json().await?;

        Ok(proposal_response)
    }

    pub async fn get_proposal_approvers(
        &self,
        context_id: &ContextId,
        proposal_id: &Hash,
    ) -> Result<GetProposalApproversResponse> {
        let url = format!(
            "{}/admin-api/contexts/{}/proposals/{}/approvals/users",
            self.base_url, context_id, proposal_id
        );

        let response = self.http_client.get(&url).send().await?;
        let approvers_response: GetProposalApproversResponse = response.json().await?;

        Ok(approvers_response)
    }

    pub async fn list_proposals(
        &self,
        context_id: &ContextId,
        args: serde_json::Value,
    ) -> Result<GetProposalsResponse> {
        let url = format!("{}/admin-api/contexts/{}/proposals", self.base_url, context_id);

        let response = self.http_client.post(&url).json(&args).send().await?;
        let proposals_response: GetProposalsResponse = response.json().await?;

        Ok(proposals_response)
    }

    pub async fn list_contexts(&self) -> Result<GetContextsResponse> {
        let url = format!("{}/admin-api/contexts", self.base_url);

        let response = self.http_client.get(&url).send().await?;
        let contexts_response: GetContextsResponse = response.json().await?;

        Ok(contexts_response)
    }

    pub async fn sync_context(&self, context_id: &ContextId) -> Result<SyncContextResponse> {
        let url = format!("{}/admin-api/contexts/sync/{}", self.base_url, context_id);

        let response = self.http_client.post(&url).json(&()).send().await?;
        let sync_response: SyncContextResponse = response.json().await?;

        Ok(sync_response)
    }

    pub async fn sync_all_contexts(&self) -> Result<SyncContextResponse> {
        let url = format!("{}/admin-api/contexts/sync", self.base_url);

        let response = self.http_client.post(&url).json(&()).send().await?;
        let sync_response: SyncContextResponse = response.json().await?;

        Ok(sync_response)
    }

    pub async fn join_context(
        &self,
        request: JoinContextRequest,
    ) -> Result<JoinContextResponse> {
        let url = format!("{}/admin-api/contexts/join", self.base_url);

        let response = self.http_client.post(&url).json(&request).send().await?;
        let join_response: JoinContextResponse = response.json().await?;

        Ok(join_response)
    }

    pub async fn get_context_identities(
        &self,
        context_id: &ContextId,
        owned: bool,
    ) -> Result<GetContextIdentitiesResponse> {
        let endpoint = if owned {
            format!("admin-api/contexts/{}/identities-owned", context_id)
        } else {
            format!("admin-api/contexts/{}/identities", context_id)
        };

        let url = format!("{}/{}", self.base_url, endpoint);
        let response = self.http_client.get(&url).send().await?;
        let identities_response: GetContextIdentitiesResponse = response.json().await?;

        Ok(identities_response)
    }

    pub async fn get_context(&self, context_id: &ContextId) -> Result<GetContextResponse> {
        let url = format!("{}/admin-api/contexts/{}", self.base_url, context_id);

        let response = self.http_client.get(&url).send().await?;
        let context_response: GetContextResponse = response.json().await?;

        Ok(context_response)
    }

    pub async fn get_context_client_keys(&self, context_id: &ContextId) -> Result<GetContextClientKeysResponse> {
        let url = format!("{}/admin-api/contexts/{}/client-keys", self.base_url, context_id);

        let response = self.http_client.get(&url).send().await?;
        let client_keys_response: GetContextClientKeysResponse = response.json().await?;

        Ok(client_keys_response)
    }

    pub async fn get_context_storage(&self, context_id: &ContextId) -> Result<GetContextStorageResponse> {
        let url = format!("{}/admin-api/contexts/{}/storage", self.base_url, context_id);

        let response = self.http_client.get(&url).send().await?;
        let storage_response: GetContextStorageResponse = response.json().await?;

        Ok(storage_response)
    }

    pub async fn delete_context(&self, context_id: &ContextId) -> Result<DeleteContextResponse> {
        let url = format!("{}/admin-api/contexts/{}", self.base_url, context_id);

        let response = self.http_client.delete(&url).send().await?;
        let delete_response: DeleteContextResponse = response.json().await?;

        Ok(delete_response)
    }
}
