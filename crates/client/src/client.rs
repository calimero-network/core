//! API client for Calimero services
//!
//! This module provides the core client functionality for making
//! authenticated API requests to Calimero services.

// Standard library
use std::str::FromStr;

// External crates
use calimero_primitives::alias::{Alias, ScopedAlias};
use calimero_primitives::application::ApplicationId;
use calimero_primitives::blobs::{BlobId, BlobInfo, BlobMetadata};
use calimero_primitives::context::ContextId;
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::PublicKey;
use calimero_server_primitives::admin::{
    AddGroupMembersApiRequest,
    AddGroupMembersApiResponse,
    AliasKind,
    CreateAliasRequest,
    CreateAliasResponse,
    CreateApplicationIdAlias,
    CreateContextIdAlias,
    CreateContextIdentityAlias,
    CreateContextRequest,
    CreateContextResponse,
    CreateGroupApiRequest,
    CreateGroupApiResponse,
    CreateGroupInvitationApiRequest,
    CreateGroupInvitationApiResponse,
    DeleteAliasResponse,
    DeleteContextApiRequest,
    DeleteContextResponse,
    DeleteGroupApiRequest,
    DeleteGroupApiResponse,
    DetachContextFromGroupApiRequest,
    DetachContextFromGroupApiResponse,
    GenerateContextIdentityResponse,
    GetApplicationResponse,
    // Group permissions
    GetContextAllowlistApiResponse,
    GetContextClientKeysResponse,
    GetContextIdentitiesResponse,
    GetContextResponse,
    GetContextStorageResponse,
    GetContextVisibilityApiResponse,
    GetContextsResponse,
    GetGroupUpgradeStatusApiResponse,
    GetLatestVersionResponse,
    GetMemberCapabilitiesApiResponse,
    GetPeersCountResponse,
    GetProposalApproversResponse,
    GetProposalResponse,
    GetProposalsResponse,
    GrantPermissionResponse,
    GroupInfoApiResponse,
    InstallApplicationRequest,
    InstallApplicationResponse,
    InstallDevApplicationRequest,
    InviteSpecializedNodeRequest,
    InviteSpecializedNodeResponse,
    InviteToContextOpenInvitationRequest,
    InviteToContextOpenInvitationResponse,
    InviteToContextRequest,
    InviteToContextResponse,
    JoinContextByOpenInvitationRequest,
    JoinContextRequest,
    JoinContextResponse,
    JoinGroupApiRequest,
    JoinGroupApiResponse,
    JoinGroupContextApiRequest,
    JoinGroupContextApiResponse,
    ListAliasesResponse,
    ListAllGroupsApiResponse,
    ListApplicationsResponse,
    ListGroupContextsApiResponse,
    ListGroupMembersApiResponse,
    ListPackagesResponse,
    ListVersionsResponse,
    LookupAliasResponse,
    ManageContextAllowlistApiRequest,
    ManageContextAllowlistApiResponse,
    RegisterGroupSigningKeyApiRequest,
    RegisterGroupSigningKeyApiResponse,
    RemoveGroupMembersApiRequest,
    RemoveGroupMembersApiResponse,
    RetryGroupUpgradeApiRequest,
    RevokePermissionResponse,
    SetContextVisibilityApiRequest,
    SetContextVisibilityApiResponse,
    SetDefaultCapabilitiesApiRequest,
    SetDefaultCapabilitiesApiResponse,
    SetDefaultVisibilityApiRequest,
    SetDefaultVisibilityApiResponse,
    SetMemberCapabilitiesApiRequest,
    SetMemberCapabilitiesApiResponse,
    SyncContextResponse,
    SyncGroupApiRequest,
    SyncGroupApiResponse,
    UninstallApplicationResponse,
    UpdateContextApplicationRequest,
    UpdateContextApplicationResponse,
    UpdateGroupSettingsApiRequest,
    UpdateGroupSettingsApiResponse,
    UpdateMemberRoleApiRequest,
    UpdateMemberRoleApiResponse,
    UpgradeGroupApiRequest,
    UpgradeGroupApiResponse,
};
use calimero_server_primitives::blob::{BlobDeleteResponse, BlobInfoResponse, BlobListResponse};
use calimero_server_primitives::jsonrpc::{Request, Response};
use eyre::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;
use url::Url;

// Local crate
use crate::connection::ConnectionInfo;
use crate::traits::{ClientAuthenticator, ClientStorage};

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
        context.map(ContextId::as_str)
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

/// Generic API client that can work with any authenticator and storage implementation
#[derive(Clone, Debug)]
pub struct Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    connection: ConnectionInfo<A, S>,
}

impl<A, S> Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub fn new(connection: ConnectionInfo<A, S>) -> Result<Self> {
        Ok(Self { connection })
    }

    pub fn api_url(&self) -> &Url {
        &self.connection.api_url
    }

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

    pub async fn delete_blob(&self, blob_id: &BlobId) -> Result<BlobDeleteResponse> {
        let response = self
            .connection
            .delete(&format!("admin-api/blobs/{blob_id}"))
            .await?;
        Ok(response)
    }

    pub async fn list_blobs(&self) -> Result<BlobListResponse> {
        let response = self.connection.get("admin-api/blobs").await?;
        Ok(response)
    }

    pub async fn get_blob_info(&self, blob_id: &BlobId) -> Result<BlobInfoResponse> {
        let headers = self
            .connection
            .head(&format!("admin-api/blobs/{blob_id}"))
            .await?;

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

    pub async fn upload_blob(
        &self,
        data: Vec<u8>,
        context_id: Option<&ContextId>,
    ) -> Result<BlobInfo> {
        let path = if let Some(ctx_id) = context_id {
            format!("admin-api/blobs?context_id={}", ctx_id)
        } else {
            "admin-api/blobs".to_owned()
        };

        let response = self.connection.put_binary(&path, data).await?;

        #[derive(serde::Deserialize)]
        struct BlobUploadResponse {
            data: BlobInfo,
        }

        let upload_response: BlobUploadResponse = response.json().await?;
        Ok(upload_response.data)
    }

    pub async fn download_blob(
        &self,
        blob_id: &BlobId,
        context_id: Option<&ContextId>,
    ) -> Result<Vec<u8>> {
        let path = if let Some(ctx_id) = context_id {
            format!("admin-api/blobs/{}?context_id={}", blob_id, ctx_id)
        } else {
            format!("admin-api/blobs/{}", blob_id)
        };

        let data = self.connection.get_binary(&path).await?;
        Ok(data)
    }

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

    pub async fn execute_jsonrpc<P>(&self, request: Request<P>) -> Result<Response>
    where
        P: Serialize,
    {
        // Debug: Print the request being sent
        eprintln!(
            "🔍 JSON-RPC Request to {}: {}",
            self.connection.api_url.join("jsonrpc")?,
            serde_json::to_string_pretty(&request)?
        );

        let response = self.connection.post("jsonrpc", request).await?;

        // Debug: Print the parsed response
        eprintln!(
            "🔍 JSON-RPC Parsed Response: {}",
            serde_json::to_string_pretty(&response)?
        );

        Ok(response)
    }

    pub async fn grant_permissions(
        &self,
        context_id: &ContextId,
        request: Vec<(PublicKey, calimero_context_config::types::Capability)>,
    ) -> Result<GrantPermissionResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/contexts/{}/capabilities/grant", context_id),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn revoke_permissions(
        &self,
        context_id: &ContextId,
        request: Vec<(PublicKey, calimero_context_config::types::Capability)>,
    ) -> Result<RevokePermissionResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/contexts/{}/capabilities/revoke", context_id),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn invite_to_context(
        &self,
        request: InviteToContextRequest,
    ) -> Result<InviteToContextResponse> {
        let response = self
            .connection
            .post("admin-api/contexts/invite", request)
            .await?;
        Ok(response)
    }

    pub async fn invite_to_context_by_open_invitation(
        &self,
        request: InviteToContextOpenInvitationRequest,
    ) -> Result<InviteToContextOpenInvitationResponse> {
        let response = self
            .connection
            .post("admin-api/contexts/invite_by_open_invitation", request)
            .await?;
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

    pub async fn get_proposal(
        &self,
        context_id: &ContextId,
        proposal_id: &Hash,
    ) -> Result<GetProposalResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/contexts/{}/proposals/{}",
                context_id, proposal_id
            ))
            .await?;
        Ok(response)
    }

    pub async fn get_proposal_approvers(
        &self,
        context_id: &ContextId,
        proposal_id: &Hash,
    ) -> Result<GetProposalApproversResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/contexts/{}/proposals/{}/approvals/users",
                context_id, proposal_id
            ))
            .await?;
        Ok(response)
    }

    pub async fn create_and_approve_proposal(
        &self,
        context_id: &ContextId,
        request: calimero_server_primitives::admin::CreateAndApproveProposalRequest,
    ) -> Result<calimero_server_primitives::admin::CreateAndApproveProposalResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/contexts/{context_id}/proposals/create-and-approve"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn approve_proposal(
        &self,
        context_id: &ContextId,
        request: calimero_server_primitives::admin::ApproveProposalRequest,
    ) -> Result<calimero_server_primitives::admin::ApproveProposalResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/contexts/{context_id}/proposals/approve"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn list_proposals(
        &self,
        context_id: &ContextId,
        args: serde_json::Value,
    ) -> Result<GetProposalsResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/contexts/{context_id}/proposals"), args)
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

    pub async fn join_context(&self, request: JoinContextRequest) -> Result<JoinContextResponse> {
        let response = self
            .connection
            .post("admin-api/contexts/join", request)
            .await?;
        Ok(response)
    }

    pub async fn join_context_by_open_invitation(
        &self,
        request: JoinContextByOpenInvitationRequest,
    ) -> Result<JoinContextResponse> {
        let response = self
            .connection
            .post("admin-api/contexts/join_by_open_invitation", request)
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
            format!("admin-api/contexts/{}/identities-owned", context_id)
        } else {
            format!("admin-api/contexts/{}/identities", context_id)
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

    /// Create context identity alias (legacy method for backward compatibility)
    pub async fn create_context_identity_alias(
        &self,
        context_id: &ContextId,
        request: CreateAliasRequest<PublicKey>,
    ) -> Result<CreateAliasResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/alias/create/identity/{}", context_id),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Create alias generic (legacy method for backward compatibility)
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
        self.create_alias(alias, value, scope).await
    }

    pub async fn create_alias<T>(
        &self,
        alias: Alias<T>,
        value: T,
        scope: Option<T::Scope>,
    ) -> Result<CreateAliasResponse>
    where
        T: UrlFragment + Serialize,
        T::Value: Serialize,
    {
        let prefix = "admin-api/alias/create";
        let kind = T::KIND;
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let body = CreateAliasRequest {
            alias,
            value: value.create(),
        };

        let response = self
            .connection
            .post(&format!("{prefix}/{kind}{scope_path}"), body)
            .await?;
        Ok(response)
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
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let response = self
            .connection
            .post_no_body(&format!("{prefix}/{kind}{scope_path}/{alias}"))
            .await?;
        Ok(response)
    }

    pub async fn list_aliases<T>(&self, scope: Option<T::Scope>) -> Result<ListAliasesResponse<T>>
    where
        T: Ord + UrlFragment + DeserializeOwned,
    {
        let prefix = "admin-api/alias/list";
        let kind = T::KIND;
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let response = self
            .connection
            .get(&format!("{prefix}/{kind}{scope_path}"))
            .await?;
        Ok(response)
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
        let scope_path = T::scoped(scope.as_ref())
            .map(|scope| format!("/{}", scope))
            .unwrap_or_default();

        let response = self
            .connection
            .post_no_body(&format!("{prefix}/{kind}{scope_path}/{alias}"))
            .await?;
        Ok(response)
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

    // Package management methods
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

    // ---- Group API methods ----

    pub async fn list_groups(&self) -> Result<ListAllGroupsApiResponse> {
        let response = self.connection.get("admin-api/groups").await?;
        Ok(response)
    }

    pub async fn create_group(
        &self,
        request: CreateGroupApiRequest,
    ) -> Result<CreateGroupApiResponse> {
        let response = self.connection.post("admin-api/groups", request).await?;
        Ok(response)
    }

    pub async fn get_group_info(&self, group_id: &str) -> Result<GroupInfoApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}"))
            .await?;
        Ok(response)
    }

    pub async fn delete_group(
        &self,
        group_id: &str,
        request: DeleteGroupApiRequest,
    ) -> Result<DeleteGroupApiResponse> {
        let response = self
            .connection
            .delete_with_body(&format!("admin-api/groups/{group_id}"), request)
            .await?;
        Ok(response)
    }

    pub async fn update_group_settings(
        &self,
        group_id: &str,
        request: UpdateGroupSettingsApiRequest,
    ) -> Result<UpdateGroupSettingsApiResponse> {
        let response = self
            .connection
            .patch(&format!("admin-api/groups/{group_id}"), request)
            .await?;
        Ok(response)
    }

    pub async fn list_group_members(&self, group_id: &str) -> Result<ListGroupMembersApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/members"))
            .await?;
        Ok(response)
    }

    pub async fn add_group_members(
        &self,
        group_id: &str,
        request: AddGroupMembersApiRequest,
    ) -> Result<AddGroupMembersApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/members"), request)
            .await?;
        Ok(response)
    }

    pub async fn remove_group_members(
        &self,
        group_id: &str,
        request: RemoveGroupMembersApiRequest,
    ) -> Result<RemoveGroupMembersApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/members/remove"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn update_member_role(
        &self,
        group_id: &str,
        identity_hex: &str,
        request: UpdateMemberRoleApiRequest,
    ) -> Result<UpdateMemberRoleApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/members/{identity_hex}/role"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn list_group_contexts(
        &self,
        group_id: &str,
    ) -> Result<ListGroupContextsApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/contexts"))
            .await?;
        Ok(response)
    }

    pub async fn detach_context_from_group(
        &self,
        group_id: &str,
        context_id: &str,
        request: DetachContextFromGroupApiRequest,
    ) -> Result<DetachContextFromGroupApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/contexts/{context_id}/remove"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn create_group_invitation(
        &self,
        group_id: &str,
        request: CreateGroupInvitationApiRequest,
    ) -> Result<CreateGroupInvitationApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/invite"), request)
            .await?;
        Ok(response)
    }

    pub async fn join_group(&self, request: JoinGroupApiRequest) -> Result<JoinGroupApiResponse> {
        let response = self
            .connection
            .post("admin-api/groups/join", request)
            .await?;
        Ok(response)
    }

    pub async fn register_group_signing_key(
        &self,
        group_id: &str,
        request: RegisterGroupSigningKeyApiRequest,
    ) -> Result<RegisterGroupSigningKeyApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/signing-key"), request)
            .await?;
        Ok(response)
    }

    pub async fn upgrade_group(
        &self,
        group_id: &str,
        request: UpgradeGroupApiRequest,
    ) -> Result<UpgradeGroupApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/upgrade"), request)
            .await?;
        Ok(response)
    }

    pub async fn get_group_upgrade_status(
        &self,
        group_id: &str,
    ) -> Result<GetGroupUpgradeStatusApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/groups/{group_id}/upgrade/status"))
            .await?;
        Ok(response)
    }

    pub async fn retry_group_upgrade(
        &self,
        group_id: &str,
        request: RetryGroupUpgradeApiRequest,
    ) -> Result<UpgradeGroupApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/upgrade/retry"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn sync_group(
        &self,
        group_id: &str,
        request: SyncGroupApiRequest,
    ) -> Result<SyncGroupApiResponse> {
        let response = self
            .connection
            .post(&format!("admin-api/groups/{group_id}/sync"), request)
            .await?;
        Ok(response)
    }

    pub async fn join_group_context(
        &self,
        group_id: &str,
        request: JoinGroupContextApiRequest,
    ) -> Result<JoinGroupContextApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/join-context"),
                request,
            )
            .await?;
        Ok(response)
    }

    // ---- Group Permissions ----

    pub async fn set_member_capabilities(
        &self,
        group_id: &str,
        identity_hex: &str,
        request: SetMemberCapabilitiesApiRequest,
    ) -> Result<SetMemberCapabilitiesApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/members/{identity_hex}/capabilities"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn get_member_capabilities(
        &self,
        group_id: &str,
        identity_hex: &str,
    ) -> Result<GetMemberCapabilitiesApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/groups/{group_id}/members/{identity_hex}/capabilities"
            ))
            .await?;
        Ok(response)
    }

    pub async fn set_context_visibility(
        &self,
        group_id: &str,
        context_id: &str,
        request: SetContextVisibilityApiRequest,
    ) -> Result<SetContextVisibilityApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/contexts/{context_id}/visibility"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn get_context_visibility(
        &self,
        group_id: &str,
        context_id: &str,
    ) -> Result<GetContextVisibilityApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/groups/{group_id}/contexts/{context_id}/visibility"
            ))
            .await?;
        Ok(response)
    }

    pub async fn manage_context_allowlist(
        &self,
        group_id: &str,
        context_id: &str,
        request: ManageContextAllowlistApiRequest,
    ) -> Result<ManageContextAllowlistApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/groups/{group_id}/contexts/{context_id}/allowlist"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn get_context_allowlist(
        &self,
        group_id: &str,
        context_id: &str,
    ) -> Result<GetContextAllowlistApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/groups/{group_id}/contexts/{context_id}/allowlist"
            ))
            .await?;
        Ok(response)
    }

    pub async fn set_default_capabilities(
        &self,
        group_id: &str,
        request: SetDefaultCapabilitiesApiRequest,
    ) -> Result<SetDefaultCapabilitiesApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/settings/default-capabilities"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn set_default_visibility(
        &self,
        group_id: &str,
        request: SetDefaultVisibilityApiRequest,
    ) -> Result<SetDefaultVisibilityApiResponse> {
        let response = self
            .connection
            .put_json(
                &format!("admin-api/groups/{group_id}/settings/default-visibility"),
                request,
            )
            .await?;
        Ok(response)
    }
}
