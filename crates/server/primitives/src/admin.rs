use std::collections::BTreeMap;

use calimero_context_config::repr::Repr;
use calimero_context_config::types::{Capability, ContextIdentity, ContextStorageEntry};
use calimero_context_config::{Proposal, ProposalWithApprovals};
use calimero_primitives::alias::Alias;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{ClientKey, ContextUser, PublicKey, WalletType};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct Empty;

// -------------------------------------------- Application API --------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApplicationRequest {
    pub url: Url,
    pub hash: Option<Hash>,
    pub metadata: Vec<u8>,
}

impl InstallApplicationRequest {
    pub const fn new(url: Url, hash: Option<Hash>, metadata: Vec<u8>) -> Self {
        Self {
            url,
            hash,
            metadata,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInstallResponseData {
    pub application_id: ApplicationId,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApplicationResponse {
    pub data: ApplicationInstallResponseData,
}

impl InstallApplicationResponse {
    pub const fn new(application_id: ApplicationId) -> Self {
        Self {
            data: ApplicationInstallResponseData { application_id },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallDevApplicationRequest {
    pub path: Utf8PathBuf,
    pub metadata: Vec<u8>,
}

impl InstallDevApplicationRequest {
    pub const fn new(path: Utf8PathBuf, metadata: Vec<u8>) -> Self {
        Self { path, metadata }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallApplicationResponseData {
    pub application_id: ApplicationId,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UninstallApplicationResponse {
    pub data: UninstallApplicationResponseData,
}

impl UninstallApplicationResponse {
    pub const fn new(application_id: ApplicationId) -> Self {
        Self {
            data: UninstallApplicationResponseData { application_id },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListApplicationResponseData {
    pub apps: Vec<Application>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListApplicationsResponse {
    pub data: ListApplicationResponseData,
}

impl ListApplicationsResponse {
    pub const fn new(apps: Vec<Application>) -> Self {
        Self {
            data: ListApplicationResponseData { apps },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetApplicationResponseData {
    pub application: Option<Application>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetApplicationResponse {
    pub data: GetApplicationResponseData,
}

impl GetApplicationResponse {
    pub const fn new(application: Option<Application>) -> Self {
        Self {
            data: GetApplicationResponseData { application },
        }
    }
}
// -------------------------------------------- Context API --------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextRequest {
    pub protocol: String,
    pub application_id: ApplicationId,
    pub context_seed: Option<Hash>,
    pub initialization_params: Vec<u8>,
}

impl CreateContextRequest {
    pub const fn new(
        protocol: String,
        application_id: ApplicationId,
        context_seed: Option<Hash>,
        initialization_params: Vec<u8>,
    ) -> Self {
        Self {
            protocol,
            application_id,
            context_seed,
            initialization_params,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextResponse {
    pub data: CreateContextResponseData,
}

impl CreateContextResponse {
    pub const fn new(context_id: ContextId, member_public_key: PublicKey) -> Self {
        Self {
            data: CreateContextResponseData {
                context_id,
                member_public_key,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeletedContextResponseData {
    pub is_deleted: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteContextResponse {
    pub data: DeletedContextResponseData,
}

impl DeleteContextResponse {
    pub const fn new(is_deleted: bool) -> Self {
        Self {
            data: DeletedContextResponseData { is_deleted },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextResponse {
    pub data: Context,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextStorageResponseData {
    pub size_in_bytes: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextStorageResponse {
    pub data: GetContextStorageResponseData,
}

impl GetContextStorageResponse {
    pub const fn new(size_in_bytes: u64) -> Self {
        Self {
            data: GetContextStorageResponseData { size_in_bytes },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextIdentitiesResponseData {
    pub identities: Vec<PublicKey>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextIdentitiesResponse {
    pub data: ContextIdentitiesResponseData,
}

impl GetContextIdentitiesResponse {
    pub const fn new(identities: Vec<PublicKey>) -> Self {
        Self {
            data: ContextIdentitiesResponseData { identities },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAliasesResponse<T> {
    #[serde(bound(deserialize = "T: Ord + Deserialize<'de>"))]
    pub data: BTreeMap<Alias<T>, T>,
}

impl<T> ListAliasesResponse<T> {
    pub fn new(data: BTreeMap<Alias<T>, T>) -> Self {
        Self { data }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextClientKeysResponseData {
    pub client_keys: Vec<ClientKey>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextClientKeysResponse {
    pub data: GetContextClientKeysResponseData,
}

impl GetContextClientKeysResponse {
    pub const fn new(client_keys: Vec<ClientKey>) -> Self {
        Self {
            data: GetContextClientKeysResponseData { client_keys },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextUsersResponseData {
    pub context_users: Vec<ContextUser>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextUsersResponse {
    pub data: GetContextUsersResponseData,
}

impl GetContextUsersResponse {
    pub const fn new(context_users: Vec<ContextUser>) -> Self {
        Self {
            data: GetContextUsersResponseData { context_users },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextsResponseData {
    pub contexts: Vec<Context>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextsResponse {
    pub data: GetContextsResponseData,
}

impl GetContextsResponse {
    pub const fn new(contexts: Vec<Context>) -> Self {
        Self {
            data: GetContextsResponseData { contexts },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteToContextRequest {
    pub context_id: ContextId,
    pub inviter_id: PublicKey,
    pub invitee_id: PublicKey,
}

impl InviteToContextRequest {
    pub const fn new(context_id: ContextId, inviter_id: PublicKey, invitee_id: PublicKey) -> Self {
        Self {
            context_id,
            inviter_id,
            invitee_id,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteToContextResponse {
    pub data: Option<ContextInvitationPayload>,
}

impl InviteToContextResponse {
    pub const fn new(payload: Option<ContextInvitationPayload>) -> Self {
        Self { data: payload }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextRequest {
    pub invitation_payload: ContextInvitationPayload,
}

impl JoinContextRequest {
    pub const fn new(invitation_payload: ContextInvitationPayload) -> Self {
        Self { invitation_payload }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextResponse {
    pub data: Option<JoinContextResponseData>,
}

impl JoinContextResponse {
    pub fn new(data: Option<(ContextId, PublicKey)>) -> Self {
        Self {
            data: data.map(|(context_id, member_public_key)| JoinContextResponseData {
                context_id,
                member_public_key,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContextApplicationRequest {
    pub application_id: ApplicationId,
    pub executor_public_key: PublicKey,
}

impl UpdateContextApplicationRequest {
    pub const fn new(application_id: ApplicationId, executor_public_key: PublicKey) -> Self {
        Self {
            application_id,
            executor_public_key,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContextApplicationResponse {
    pub data: Empty,
}

impl UpdateContextApplicationResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

// -------------------------------------------- Identity API ----------------------------------------
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContextIdentityResponseData {
    pub public_key: PublicKey,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateContextIdentityResponse {
    pub data: GenerateContextIdentityResponseData,
}

impl GenerateContextIdentityResponse {
    pub const fn new(public_key: PublicKey) -> Self {
        Self {
            data: GenerateContextIdentityResponseData { public_key },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CreateAliasRequest<T: AliasKind> {
    pub alias: Alias<T>,
    #[serde(flatten)]
    pub value: T::Value,
}

pub trait AliasKind {
    type Value;

    fn from_value(data: Self::Value) -> Self;
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextIdAlias {
    pub context_id: ContextId,
}

impl AliasKind for ContextId {
    type Value = CreateContextIdAlias;

    fn from_value(data: Self::Value) -> Self {
        data.context_id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct CreateContextIdentityAlias {
    pub identity: PublicKey,
}

impl AliasKind for PublicKey {
    type Value = CreateContextIdentityAlias;

    fn from_value(data: Self::Value) -> Self {
        data.identity
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateApplicationIdAlias {
    pub application_id: ApplicationId,
}

impl AliasKind for ApplicationId {
    type Value = CreateApplicationIdAlias;

    fn from_value(data: Self::Value) -> Self {
        data.application_id
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAliasResponse {
    pub data: Empty,
}

impl CreateAliasResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAliasResponse {
    pub data: Empty,
}

impl DeleteAliasResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupAliasResponse<T> {
    pub data: LookupAliasResponseData<T>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LookupAliasResponseData<T> {
    pub value: Option<T>,
}

impl<T> LookupAliasResponseData<T> {
    pub const fn new(value: Option<T>) -> Self {
        Self { value }
    }
}

// -------------------------------------------- Misc API --------------------------------------------

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct GetPeersCountResponse {
    pub count: usize,
}

impl GetPeersCountResponse {
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self { count }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AddPublicKeyRequest {
    pub wallet_signature: WalletSignature,
    pub payload: Payload,
    pub wallet_metadata: WalletMetadata,
    pub context_id: Option<ContextId>,
}

impl AddPublicKeyRequest {
    #[must_use]
    pub const fn new(
        wallet_signature: WalletSignature,
        payload: Payload,
        wallet_metadata: WalletMetadata,
        context_id: Option<ContextId>,
    ) -> Self {
        Self {
            wallet_signature,
            payload,
            wallet_metadata,
            context_id,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Payload {
    pub message: SignatureMessage,
    pub metadata: SignatureMetadataEnum,
}

impl Payload {
    #[must_use]
    pub const fn new(message: SignatureMessage, metadata: SignatureMetadataEnum) -> Self {
        Self { message, metadata }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct SignatureMessage {
    pub context_id: Option<ContextId>,
    pub nonce: String,
    pub timestamp: i64,
    pub node_signature: String,
    pub message: String,
    pub public_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct WalletMetadata {
    #[serde(rename = "wallet")]
    pub wallet_type: WalletType,
    pub verifying_key: String,
    pub wallet_address: Option<String>,
    pub network_metadata: Option<NetworkMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NetworkMetadata {
    pub chain_id: String,
    pub rpc_url: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum SignatureMetadataEnum {
    NEAR(NearSignatureMessageMetadata),
    ETH(EthSignatureMessageMetadata),
    STARKNET(StarknetSignatureMessageMetadata),
    ICP(ICPSignatureMessageMetadata),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NearSignatureMessageMetadata {
    pub recipient: String,
    pub callback_url: String,
    pub nonce: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "Needed for serialisation"
)]
pub struct EthSignatureMessageMetadata {}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "Needed for serialisation"
)]
pub struct StarknetSignatureMessageMetadata {}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[expect(clippy::exhaustive_structs, reason = "Considered to be exhaustive")]
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "Needed for serialisation"
)]
pub struct ICPSignatureMessageMetadata {}

// Intermediate structs for initial parsing
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct IntermediateAddPublicKeyRequest {
    pub wallet_signature: WalletSignature,
    pub payload: IntermediatePayload,
    pub wallet_metadata: WalletMetadata, // Reuse WalletMetadata as it fits the intermediate step
    pub context_id: Option<ContextId>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum WalletSignature {
    String(String),
    StarknetPayload(StarknetPayload),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct JwtTokenRequest {
    pub context_id: ContextId,
    pub executor_public_key: String,
}

impl JwtTokenRequest {
    #[must_use]
    pub const fn new(context_id: ContextId, executor_public_key: String) -> Self {
        Self {
            context_id,
            executor_public_key,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct JwtRefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct StarknetPayload {
    pub signature: Vec<String>,
    pub message_hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct IntermediatePayload {
    pub message: SignatureMessage, // Reuse SignatureMessage as it fits the intermediate step
    pub metadata: Value,           // Raw JSON value for the metadata
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NodeChallenge {
    #[serde(flatten)]
    pub message: NodeChallengeMessage,
    pub node_signature: String,
}

impl NodeChallenge {
    #[must_use]
    pub const fn new(message: NodeChallengeMessage, node_signature: String) -> Self {
        Self {
            message,
            node_signature,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NodeChallengeMessage {
    pub nonce: String,
    pub context_id: Option<ContextId>,
    pub timestamp: i64,
}

impl NodeChallengeMessage {
    #[must_use]
    pub const fn new(nonce: String, context_id: Option<ContextId>, timestamp: i64) -> Self {
        Self {
            nonce,
            context_id,
            timestamp,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalsResponse {
    pub data: Vec<Proposal>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalResponse {
    pub data: Proposal,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProxyContractResponse {
    pub data: String,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalsRequest {
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextValueRequest {
    pub key: String,
}

#[derive(Debug, Deserialize, Serialize, Copy, Clone)]
#[serde(rename_all = "camelCase")]
pub struct GetContextStorageEntriesRequest {
    pub offset: usize,
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextValueResponse {
    pub data: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextStorageEntriesResponse {
    pub data: Vec<ContextStorageEntry>,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfActiveProposalsResponse {
    pub data: u16,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProposalApproversResponse {
    // fixme! this is wrong, ContextIdentity is an implementation
    // fixme! detail it should be PublicKey instead
    pub data: Vec<Repr<ContextIdentity>>,
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNumberOfProposalApprovalsResponse {
    pub data: ProposalWithApprovals,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantPermissionRequest {
    pub context_id: ContextId,
    pub granter_id: PublicKey,
    pub grantee_id: PublicKey,
    pub capability: Capability,
}

impl GrantPermissionRequest {
    pub const fn new(
        context_id: ContextId,
        granter_id: PublicKey,
        grantee_id: PublicKey,
        capability: Capability,
    ) -> Self {
        Self {
            context_id,
            granter_id,
            grantee_id,
            capability,
        }
    }
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantPermissionResponse {
    pub data: Empty,
}

impl GrantPermissionResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokePermissionRequest {
    pub context_id: ContextId,
    pub revoker_id: PublicKey,
    pub revokee_id: PublicKey,
    pub capability: Capability,
}

impl RevokePermissionRequest {
    pub const fn new(
        context_id: ContextId,
        revoker_id: PublicKey,
        revokee_id: PublicKey,
        capability: Capability,
    ) -> Self {
        Self {
            context_id,
            revoker_id,
            revokee_id,
            capability,
        }
    }
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevokePermissionResponse {
    pub data: Empty,
}

impl RevokePermissionResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncContextResponse {
    pub data: Empty,
}

impl SyncContextResponse {
    pub const fn new() -> Self {
        Self { data: Empty {} }
    }
}
