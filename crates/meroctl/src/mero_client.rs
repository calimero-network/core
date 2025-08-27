use std::str::FromStr;

use calimero_primitives::alias::{Alias, ScopedAlias};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    AliasKind, CreateAliasRequest, CreateAliasResponse, CreateApplicationIdAlias,
    CreateContextIdAlias, CreateContextIdentityAlias, CreateContextRequest, CreateContextResponse,
    DeleteAliasResponse, DeleteContextResponse, GenerateContextIdentityResponse,
    GetApplicationResponse, GetContextClientKeysResponse, GetContextIdentitiesResponse,
    GetContextResponse, GetContextStorageResponse, GetContextsResponse, GetPeersCountResponse,
    GetProposalApproversResponse, GetProposalResponse, GetProposalsResponse,
    GrantPermissionResponse, InstallApplicationRequest, InstallApplicationResponse,
    InstallDevApplicationRequest, InviteToContextRequest, InviteToContextResponse,
    JoinContextRequest, JoinContextResponse, ListAliasesResponse, ListApplicationsResponse,
    LookupAliasResponse, RevokePermissionResponse, SyncContextResponse,
    UninstallApplicationResponse, UpdateContextApplicationRequest,
    UpdateContextApplicationResponse,
};
use calimero_server_primitives::jsonrpc::{Request, Response};
use eyre::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use url::Url;

pub trait UrlFragment: ScopedAlias + AliasKind {
    const KIND: &'static str;

    fn create(self) -> Self::Value;

    fn scoped(scope: Option<&Self::Scope>) -> Option<&str>;
}

impl UrlFragment for ContextId {
    const KIND: &'static str = "context";

    fn create(self) -> Self::Value {
        CreateContextIdAlias { context_id: self }
    }

    fn scoped(_: Option<&Self::Scope>) -> Option<&str> {
        None
    }
}

impl UrlFragment for PublicKey {
    const KIND: &'static str = "identity";

    fn create(self) -> Self::Value {
        CreateContextIdentityAlias { identity: self }
    }

    fn scoped(context: Option<&Self::Scope>) -> Option<&str> {
        context.map(calimero_primitives::context::ContextId::as_str)
    }
}

impl UrlFragment for ApplicationId {
    const KIND: &'static str = "application";

    fn create(self) -> Self::Value {
        CreateApplicationIdAlias {
            application_id: self,
        }
    }

    fn scoped(_: Option<&Self::Scope>) -> Option<&str> {
        None
    }
}

#[derive(Debug, Serialize)]
pub struct ResolveResponse<T> {
    alias: Alias<T>,
    value: Option<ResolveResponseValue<T>>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", content = "data")]
pub enum ResolveResponseValue<T> {
    Lookup(LookupAliasResponse<T>),
    Parsed(T),
}

impl<T> ResolveResponse<T> {
    pub fn value(&self) -> Option<&T> {
        match self.value.as_ref()? {
            ResolveResponseValue::Lookup(value) => value.data.value.as_ref(),
            ResolveResponseValue::Parsed(value) => Some(value),
        }
    }

    pub fn alias(&self) -> &Alias<T> {
        &self.alias
    }

    pub fn value_enum(&self) -> Option<&ResolveResponseValue<T>> {
        self.value.as_ref()
    }
}

#[derive(Debug, Serialize, Deserialize, Copy, Clone)]
pub struct BlobDeleteResponse {
    pub blob_id: BlobId,
    pub deleted: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponse {
    pub data: BlobListResponseData,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobListResponseData {
    pub blobs: Vec<BlobInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlobInfoResponse {
    pub data: BlobMetadata,
}

#[derive(Clone, Debug)]
pub struct MeroClient {
    base_url: Url,
    http_client: reqwest::Client,
}

impl MeroClient {
    pub fn new(base_url: String) -> Result<Self> {
        let base_url = Url::parse(&base_url)?;
        Ok(Self {
            base_url,
            http_client: reqwest::Client::new(),
        })
    }

    pub async fn get_application(&self, app_id: &ApplicationId) -> Result<GetApplicationResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/applications/{app_id}"))?;

        let response = self.http_client.get(url).send().await?;
        let application_response: GetApplicationResponse = response.json().await?;

        Ok(application_response)
    }

    pub async fn install_dev_application(
        &self,
        request: InstallDevApplicationRequest,
    ) -> Result<InstallApplicationResponse> {
        let url = self.base_url.join("admin-api/install-dev-application")?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let install_response: InstallApplicationResponse = response.json().await?;

        Ok(install_response)
    }

    pub async fn install_application(
        &self,
        request: InstallApplicationRequest,
    ) -> Result<InstallApplicationResponse> {
        let url = self.base_url.join("admin-api/install-application")?;

        let response = self.http_client.post(url).json(&request).send().await?;

        let install_response: InstallApplicationResponse = response.json().await?;
        Ok(install_response)
    }

    pub async fn list_applications(&self) -> Result<ListApplicationsResponse> {
        let url = self.base_url.join("admin-api/applications")?;

        let response = self.http_client.get(url).send().await?;

        let list_response: ListApplicationsResponse = response.json().await?;
        Ok(list_response)
    }

    pub async fn uninstall_application(
        &self,
        app_id: &ApplicationId,
    ) -> Result<UninstallApplicationResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/applications/{app_id}"))?;

        let response = self.http_client.delete(url).send().await?;
        let uninstall_response: UninstallApplicationResponse = response.json().await?;

        Ok(uninstall_response)
    }

    pub async fn delete_blob(&self, blob_id: &BlobId) -> Result<BlobDeleteResponse> {
        let url = self.base_url.join(&format!("admin-api/blobs/{blob_id}"))?;

        let response = self.http_client.delete(url).send().await?;
        let delete_response: BlobDeleteResponse = response.json().await?;

        Ok(delete_response)
    }

    pub async fn list_blobs(&self) -> Result<BlobListResponse> {
        let url = self.base_url.join("admin-api/blobs")?;

        let response = self.http_client.get(url).send().await?;

        let list_response: BlobListResponse = response.json().await?;
        Ok(list_response)
    }

    pub async fn get_blob_info(&self, blob_id: &BlobId) -> Result<BlobInfoResponse> {
        let url = self.base_url.join(&format!("admin-api/blobs/{blob_id}"))?;

        let response = self.http_client.head(url).send().await?;
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
        let url = self.base_url.join("admin-api/identity/context")?;

        let response = self.http_client.post(url).send().await?;
        let identity_response: GenerateContextIdentityResponse = response.json().await?;

        Ok(identity_response)
    }

    pub async fn get_peers_count(&self) -> Result<GetPeersCountResponse> {
        let url = self.base_url.join("admin-api/peers")?;

        let response = self.http_client.get(url).send().await?;
        let peers_response: GetPeersCountResponse = response.json().await?;

        Ok(peers_response)
    }

    pub async fn execute_jsonrpc<P>(&self, request: Request<P>) -> Result<Response>
    where
        P: Serialize,
    {
        let url = self.base_url.join("jsonrpc")?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let jsonrpc_response: Response = response.json().await?;

        Ok(jsonrpc_response)
    }

    pub async fn grant_permissions(
        &self,
        context_id: &ContextId,
        request: Vec<(PublicKey, calimero_context_config::types::Capability)>,
    ) -> Result<GrantPermissionResponse> {
        let url = self.base_url.join(&format!(
            "admin-api/contexts/{}/capabilities/grant",
            context_id
        ))?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let grant_response: GrantPermissionResponse = response.json().await?;

        Ok(grant_response)
    }

    pub async fn revoke_permissions(
        &self,
        context_id: &ContextId,
        request: Vec<(PublicKey, calimero_context_config::types::Capability)>,
    ) -> Result<RevokePermissionResponse> {
        let url = self.base_url.join(&format!(
            "admin-api/contexts/{}/capabilities/revoke",
            context_id
        ))?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let revoke_response: RevokePermissionResponse = response.json().await?;

        Ok(revoke_response)
    }

    pub async fn invite_to_context(
        &self,
        request: InviteToContextRequest,
    ) -> Result<InviteToContextResponse> {
        let url = self.base_url.join("admin-api/contexts/invite")?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let invite_response: InviteToContextResponse = response.json().await?;

        Ok(invite_response)
    }

    pub async fn update_context_application(
        &self,
        context_id: &ContextId,
        request: UpdateContextApplicationRequest,
    ) -> Result<UpdateContextApplicationResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/contexts/{context_id}/application"))?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let update_response: UpdateContextApplicationResponse = response.json().await?;

        Ok(update_response)
    }

    pub async fn get_proposal(
        &self,
        context_id: &ContextId,
        proposal_id: &Hash,
    ) -> Result<GetProposalResponse> {
        let url = self.base_url.join(&format!(
            "admin-api/contexts/{}/proposals/{}",
            context_id, proposal_id
        ))?;

        let response = self.http_client.get(url).send().await?;
        let proposal_response: GetProposalResponse = response.json().await?;

        Ok(proposal_response)
    }

    pub async fn get_proposal_approvers(
        &self,
        context_id: &ContextId,
        proposal_id: &Hash,
    ) -> Result<GetProposalApproversResponse> {
        let url = self.base_url.join(&format!(
            "admin-api/contexts/{}/proposals/{}/approvals/users",
            context_id, proposal_id
        ))?;

        let response = self.http_client.get(url).send().await?;
        let approvers_response: GetProposalApproversResponse = response.json().await?;

        Ok(approvers_response)
    }

    pub async fn list_proposals(
        &self,
        context_id: &ContextId,
        args: serde_json::Value,
    ) -> Result<GetProposalsResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/contexts/{context_id}/proposals"))?;

        let response = self.http_client.post(url).json(&args).send().await?;
        let proposals_response: GetProposalsResponse = response.json().await?;

        Ok(proposals_response)
    }

    pub async fn list_contexts(&self) -> Result<GetContextsResponse> {
        let url = self.base_url.join("admin-api/contexts")?;

        let response = self.http_client.get(url).send().await?;
        let contexts_response: GetContextsResponse = response.json().await?;

        Ok(contexts_response)
    }

    pub async fn sync_context(&self, context_id: &ContextId) -> Result<SyncContextResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/contexts/sync/{context_id}"))?;

        let response = self.http_client.post(url).json(&()).send().await?;
        let sync_response: SyncContextResponse = response.json().await?;

        Ok(sync_response)
    }

    pub async fn sync_all_contexts(&self) -> Result<SyncContextResponse> {
        let url = self.base_url.join("admin-api/contexts/sync")?;

        let response = self.http_client.post(url).json(&()).send().await?;
        let sync_response: SyncContextResponse = response.json().await?;

        Ok(sync_response)
    }

    pub async fn join_context(&self, request: JoinContextRequest) -> Result<JoinContextResponse> {
        let url = self.base_url.join("admin-api/contexts/join")?;

        let response = self.http_client.post(url).json(&request).send().await?;
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

        let url = self.base_url.join(&endpoint)?;
        let response = self.http_client.get(url).send().await?;
        let identities_response: GetContextIdentitiesResponse = response.json().await?;

        Ok(identities_response)
    }

    pub async fn get_context(&self, context_id: &ContextId) -> Result<GetContextResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/contexts/{}", context_id))?;

        let response = self.http_client.get(url).send().await?;
        let context_response: GetContextResponse = response.json().await?;

        Ok(context_response)
    }

    pub async fn get_context_client_keys(
        &self,
        context_id: &ContextId,
    ) -> Result<GetContextClientKeysResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/contexts/{}/client-keys", context_id))?;

        let response = self.http_client.get(url).send().await?;
        let client_keys_response: GetContextClientKeysResponse = response.json().await?;

        Ok(client_keys_response)
    }

    pub async fn get_context_storage(
        &self,
        context_id: &ContextId,
    ) -> Result<GetContextStorageResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/contexts/{}/storage", context_id))?;

        let response = self.http_client.get(url).send().await?;
        let storage_response: GetContextStorageResponse = response.json().await?;

        Ok(storage_response)
    }

    pub async fn delete_context(&self, context_id: &ContextId) -> Result<DeleteContextResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/contexts/{}", context_id))?;

        let response = self.http_client.delete(url).send().await?;
        let delete_response: DeleteContextResponse = response.json().await?;

        Ok(delete_response)
    }

    pub async fn create_context(
        &self,
        request: CreateContextRequest,
    ) -> Result<CreateContextResponse> {
        let url = self.base_url.join("admin-api/contexts")?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let create_context_response: CreateContextResponse = response.json().await?;

        Ok(create_context_response)
    }

    pub async fn create_context_identity_alias(
        &self,
        context_id: &ContextId,
        request: CreateAliasRequest<PublicKey>,
    ) -> Result<CreateAliasResponse> {
        let url = self
            .base_url
            .join(&format!("admin-api/alias/create/identity/{}", context_id))?;

        let response = self.http_client.post(url).json(&request).send().await?;
        let create_alias_response: CreateAliasResponse = response.json().await?;

        Ok(create_alias_response)
    }

    pub async fn create_alias(
        &self,
        alias: Alias<ContextId>,
        value: Option<ContextId>,
    ) -> Result<CreateAliasResponse> {
        let url = self.base_url.join("admin-api/alias/create/context")?;

        let request = CreateAliasRequest {
            alias,
            value: CreateContextIdAlias {
                context_id: value.unwrap_or_else(|| ContextId::default()),
            },
        };

        let response = self.http_client.post(url).json(&request).send().await?;
        let create_alias_response: CreateAliasResponse = response.json().await?;

        Ok(create_alias_response)
    }

    pub async fn create_alias_generic<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
        value: T,
    ) -> Result<CreateAliasResponse>
    where
        T: UrlFragment + Serialize,
        T::Value: Serialize,
    {
        let prefix = "admin-api/alias/create";
        let kind = T::KIND;
        let scope =
            T::scoped(scope.as_ref()).map_or_else(Default::default, |scope| format!("/{}", scope));

        let body = CreateAliasRequest {
            alias,
            value: value.create(),
        };

        let url = self.base_url.join(&format!("{prefix}/{kind}{scope}"))?;
        let response = self.http_client.post(url).json(&body).send().await?;
        let create_alias_response: CreateAliasResponse = response.json().await?;

        Ok(create_alias_response)
    }

    pub async fn delete_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> Result<DeleteAliasResponse>
    where
        T: UrlFragment,
    {
        let prefix = "admin-api/alias/delete";
        let kind = T::KIND;
        let scope =
            T::scoped(scope.as_ref()).map_or_else(Default::default, |scope| format!("{}/", scope));

        let url = self
            .base_url
            .join(&format!("{prefix}/{kind}/{scope}{alias}"))?;
        let response = self.http_client.post(url).send().await?;
        let delete_alias_response: DeleteAliasResponse = response.json().await?;

        Ok(delete_alias_response)
    }

    pub async fn list_aliases<T>(&self, scope: Option<T::Scope>) -> Result<ListAliasesResponse<T>>
    where
        T: Ord + UrlFragment + DeserializeOwned,
    {
        let prefix = "admin-api/alias/list";
        let kind = T::KIND;
        let scope =
            T::scoped(scope.as_ref()).map_or_else(Default::default, |scope| format!("/{}", scope));

        let url = self.base_url.join(&format!("{prefix}/{kind}{scope}"))?;
        let response = self.http_client.get(url).send().await?;
        let list_aliases_response: ListAliasesResponse<T> = response.json().await?;

        Ok(list_aliases_response)
    }

    pub async fn lookup_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> Result<LookupAliasResponse<T>>
    where
        T: UrlFragment + DeserializeOwned,
    {
        let prefix = "admin-api/alias/lookup";
        let kind = T::KIND;
        let scope =
            T::scoped(scope.as_ref()).map_or_else(Default::default, |scope| format!("{}/", scope));

        let url = self
            .base_url
            .join(&format!("{prefix}/{kind}/{scope}{alias}"))?;
        let response = self.http_client.post(url).send().await?;
        let lookup_alias_response: LookupAliasResponse<T> = response.json().await?;

        Ok(lookup_alias_response)
    }

    pub async fn resolve_alias<T>(
        &self,
        alias: Alias<T>,
        scope: Option<T::Scope>,
    ) -> Result<ResolveResponse<T>>
    where
        T: UrlFragment + FromStr + DeserializeOwned,
    {
        let value = self.lookup_alias(alias, scope).await?;

        if value.data.value.is_some() {
            return Ok(ResolveResponse {
                alias,
                value: Some(ResolveResponseValue::Lookup(value)),
            });
        }

        let value = alias
            .as_str()
            .parse()
            .ok()
            .map(ResolveResponseValue::Parsed);

        Ok(ResolveResponse { alias, value })
    }
}
