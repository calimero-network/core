use std::collections::BTreeMap;

use calimero_context_config::repr::Repr;
use calimero_context_config::types::{
    BlockHeight, Capability, ContextIdentity, ContextStorageEntry, SignedOpenInvitation,
};
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
    pub package: Option<String>,
    pub version: Option<String>,
}

impl InstallApplicationRequest {
    pub fn new(
        url: Url,
        hash: Option<Hash>,
        metadata: Vec<u8>,
        package: Option<String>,
        version: Option<String>,
    ) -> Self {
        Self {
            url,
            hash,
            metadata,
            package,
            version,
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
    pub package: Option<String>,
    pub version: Option<String>,
}

impl InstallDevApplicationRequest {
    pub fn new(
        path: Utf8PathBuf,
        metadata: Vec<u8>,
        package: Option<String>,
        version: Option<String>,
    ) -> Self {
        Self {
            path,
            metadata,
            package,
            version,
        }
    }
}

// -------------------------------------------- Bundle API --------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleArtifact {
    pub path: String,
    pub hash: Option<String>,
    pub size: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleManifest {
    pub version: String,
    pub package: String,
    pub app_version: String,
    pub wasm: Option<BundleArtifact>,
    pub abi: Option<BundleArtifact>,
    pub migrations: Vec<BundleArtifact>,
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

// -------------------------------------------- Package Management API --------------------------------------------
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListPackagesResponse {
    pub packages: Vec<String>,
}

impl ListPackagesResponse {
    pub const fn new(packages: Vec<String>) -> Self {
        Self { packages }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListVersionsResponse {
    pub versions: Vec<String>,
}

impl ListVersionsResponse {
    pub const fn new(versions: Vec<String>) -> Self {
        Self { versions }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetLatestVersionResponse {
    pub application_id: Option<ApplicationId>,
}

impl GetLatestVersionResponse {
    pub const fn new(application_id: Option<ApplicationId>) -> Self {
        Self { application_id }
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

// TODO: refactor `InviteToContextRequest` with an optional `invitee_id` field to serve both
// types of requests.
#[derive(Debug, Deserialize, Copy, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteToContextOpenInvitationRequest {
    pub context_id: ContextId,
    pub inviter_id: PublicKey,
    pub valid_for_blocks: BlockHeight,
}

impl InviteToContextOpenInvitationRequest {
    pub const fn new(
        context_id: ContextId,
        inviter_id: PublicKey,
        valid_for_blocks: BlockHeight,
    ) -> Self {
        Self {
            context_id,
            inviter_id,
            valid_for_blocks,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteToContextOpenInvitationResponse {
    pub data: Option<SignedOpenInvitation>,
}

impl InviteToContextOpenInvitationResponse {
    pub const fn new(signed_open_invitation: Option<SignedOpenInvitation>) -> Self {
        Self {
            data: signed_open_invitation,
        }
    }
}

/// Request to invite specialized nodes (e.g., read-only TEE nodes) to join a context
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteSpecializedNodeRequest {
    pub context_id: ContextId,
    /// Optional inviter identity - defaults to context's default identity if not provided
    pub inviter_id: Option<PublicKey>,
}

impl InviteSpecializedNodeRequest {
    pub const fn new(context_id: ContextId, inviter_id: Option<PublicKey>) -> Self {
        Self {
            context_id,
            inviter_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteSpecializedNodeResponseData {
    /// Hex-encoded nonce used for the specialized node invite discovery
    pub nonce: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InviteSpecializedNodeResponse {
    pub data: InviteSpecializedNodeResponseData,
}

impl InviteSpecializedNodeResponse {
    pub fn new(nonce: String) -> Self {
        Self {
            data: InviteSpecializedNodeResponseData { nonce },
        }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextByOpenInvitationRequest {
    pub invitation: SignedOpenInvitation,
    pub new_member_public_key: PublicKey,
}

impl JoinContextByOpenInvitationRequest {
    pub const fn new(invitation: SignedOpenInvitation, new_member_public_key: PublicKey) -> Self {
        Self {
            invitation,
            new_member_public_key,
        }
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAndApproveProposalRequest {
    pub signer_id: PublicKey,
    pub proposal: Proposal,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAndApproveProposalResponse {
    pub data: Option<ProposalWithApprovals>,
}

impl CreateAndApproveProposalResponse {
    pub const fn new(data: Option<ProposalWithApprovals>) -> Self {
        Self { data }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveProposalRequest {
    pub signer_id: PublicKey,
    pub proposal_id: calimero_context_config::types::ProposalId,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApproveProposalResponse {
    pub data: Option<ProposalWithApprovals>,
}

impl ApproveProposalResponse {
    pub const fn new(data: Option<ProposalWithApprovals>) -> Self {
        Self { data }
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

// -------------------------------------------- TEE API --------------------------------------------

// Serializable TDX Quote Types (mirrors tdx_quote::Quote structure)

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Quote {
    pub header: QuoteHeader,
    pub body: QuoteBody,
    pub signature: String,
    pub attestation_key: String,
    pub certification_data: CertificationData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteHeader {
    pub version: u16,
    pub attestation_key_type: u16,
    pub tee_type: u32,
    pub qe_vendor_id: String,
    pub user_data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteBody {
    /// TDX version
    pub tdx_version: String,
    /// TEE Trusted Computing Base Security Version Number (16 bytes)
    pub tee_tcb_svn: String,
    /// Measurement of SEAM module (48 bytes)
    pub mrseam: String,
    /// Measurement of SEAM signer (48 bytes)
    pub mrsignerseam: String,
    /// SEAM attributes (8 bytes)
    pub seamattributes: String,
    /// Trust Domain attributes (8 bytes)
    pub tdattributes: String,
    /// Extended features available mask (8 bytes)
    pub xfam: String,
    /// Measurement Register of Trust Domain (48 bytes) - hash of kernel + initrd + app
    pub mrtd: String,
    /// Measurement of configuration (48 bytes)
    pub mrconfigid: String,
    /// Measurement of owner (48 bytes)
    pub mrowner: String,
    /// Measurement of owner configuration (48 bytes)
    pub mrownerconfig: String,
    /// Runtime Measurement Register 0 (48 bytes)
    pub rtmr0: String,
    /// Runtime Measurement Register 1 (48 bytes)
    pub rtmr1: String,
    /// Runtime Measurement Register 2 (48 bytes)
    pub rtmr2: String,
    /// Runtime Measurement Register 3 (48 bytes)
    pub rtmr3: String,
    /// Report data (64 bytes): nonce[32] || app_hash[32]
    pub reportdata: String,
    /// Optional second TEE TCB SVN (16 bytes) - TDX 1.5+
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tee_tcb_svn_2: Option<String>,
    /// Optional measurement of service TD (48 bytes) - TDX 1.5+
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mrservicetd: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QeReportCertificationDataInfo {
    /// QE report (384 bytes hex)
    pub qe_report: String,
    /// ECDSA signature (hex)
    pub signature: String,
    /// QE authentication data (hex)
    pub qe_authentication_data: String,
    /// Inner certification data type
    pub certification_data_type: String,
    /// Inner certification data (hex)
    pub certification_data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "camelCase")]
pub enum CertificationData {
    #[serde(rename = "pckIdPpidPlainCpusvnPcesvn")]
    PckIdPpidPlainCpusvnPcesvn(String),
    #[serde(rename = "pckIdPpidRSA2048CpusvnPcesvn")]
    PckIdPpidRSA2048CpusvnPcesvn(String),
    #[serde(rename = "pckIdPpidRSA3072CpusvnPcesvn")]
    PckIdPpidRSA3072CpusvnPcesvn(String),
    #[serde(rename = "pckLeafCert")]
    PckLeafCert(String),
    #[serde(rename = "pckCertChain")]
    PckCertChain(String),
    #[serde(rename = "qeReportCertificationData")]
    QeReportCertificationData(QeReportCertificationDataInfo),
    #[serde(rename = "platformManifest")]
    PlatformManifest(String),
}

// Conversion from tdx_quote::Quote to our serializable Quote type
impl TryFrom<tdx_quote::Quote> for Quote {
    type Error = String;

    fn try_from(quote: tdx_quote::Quote) -> Result<Self, Self::Error> {
        use tdx_quote::CertificationData as TdxCert;
        use tdx_quote::CertificationDataInner;

        // Extract method results first to avoid borrow issues
        let mrtd = hex::encode(quote.mrtd());
        let rtmr0 = hex::encode(quote.rtmr0());
        let rtmr1 = hex::encode(quote.rtmr1());
        let rtmr2 = hex::encode(quote.rtmr2());
        let rtmr3 = hex::encode(quote.rtmr3());
        let reportdata = hex::encode(quote.report_input_data());

        Ok(Self {
            header: QuoteHeader {
                version: quote.header.version,
                attestation_key_type: quote.header.attestation_key_type as u16,
                tee_type: quote.header.tee_type as u32,
                qe_vendor_id: hex::encode(&quote.header.qe_vendor_id),
                user_data: hex::encode(&quote.header.user_data),
            },
            body: QuoteBody {
                tdx_version: match quote.body.tdx_version {
                    tdx_quote::TDXVersion::One => "1.0".to_string(),
                    tdx_quote::TDXVersion::OnePointFive => "1.5".to_string(),
                },
                tee_tcb_svn: hex::encode(&quote.body.tee_tcb_svn),
                mrseam: hex::encode(&quote.body.mrseam),
                mrsignerseam: hex::encode(&quote.body.mrsignerseam),
                seamattributes: hex::encode(&quote.body.seamattributes),
                tdattributes: hex::encode(&quote.body.tdattributes),
                xfam: hex::encode(&quote.body.xfam),
                mrtd,
                mrconfigid: hex::encode(&quote.body.mrconfigid),
                mrowner: hex::encode(&quote.body.mrowner),
                mrownerconfig: hex::encode(&quote.body.mrownerconfig),
                rtmr0,
                rtmr1,
                rtmr2,
                rtmr3,
                reportdata,
                tee_tcb_svn_2: quote.body.tee_tcb_svn_2.map(|v| hex::encode(&v)),
                mrservicetd: quote.body.mrservicetd.map(|v| hex::encode(&v)),
            },
            signature: hex::encode(quote.signature.to_bytes()),
            attestation_key: hex::encode(quote.attestation_key.to_sec1_bytes()),
            certification_data: match quote.certification_data {
                TdxCert::PckIdPpidPlainCpusvnPcesvn(data) => {
                    CertificationData::PckIdPpidPlainCpusvnPcesvn(hex::encode(&data))
                }
                TdxCert::PckIdPpidRSA2048CpusvnPcesvn(data) => {
                    CertificationData::PckIdPpidRSA2048CpusvnPcesvn(hex::encode(&data))
                }
                TdxCert::PckIdPpidRSA3072CpusvnPcesvn(data) => {
                    CertificationData::PckIdPpidRSA3072CpusvnPcesvn(hex::encode(&data))
                }
                TdxCert::PckLeafCert(data) => CertificationData::PckLeafCert(hex::encode(&data)),
                TdxCert::PckCertChain(data) => CertificationData::PckCertChain(hex::encode(&data)),
                TdxCert::QeReportCertificationData(data) => {
                    // Properly serialize the nested QeReportCertificationData structure
                    let (cert_type, cert_data) = match &data.certification_data {
                        CertificationDataInner::PckIdPpidPlainCpusvnPcesvn(d) => {
                            ("PckIdPpidPlainCpusvnPcesvn", hex::encode(d))
                        }
                        CertificationDataInner::PckIdPpidRSA2048CpusvnPcesvn(d) => {
                            ("PckIdPpidRSA2048CpusvnPcesvn", hex::encode(d))
                        }
                        CertificationDataInner::PckIdPpidRSA3072CpusvnPcesvn(d) => {
                            ("PckIdPpidRSA3072CpusvnPcesvn", hex::encode(d))
                        }
                        CertificationDataInner::PckLeafCert(d) => ("PckLeafCert", hex::encode(d)),
                        CertificationDataInner::PckCertChain(d) => ("PckCertChain", hex::encode(d)),
                        CertificationDataInner::PlatformManifest(d) => {
                            ("PlatformManifest", hex::encode(d))
                        }
                        // Return error for unknown inner certification data variants
                        _ => {
                            return Err(
                                "Unknown CertificationDataInner variant encountered".to_string()
                            )
                        }
                    };

                    CertificationData::QeReportCertificationData(QeReportCertificationDataInfo {
                        qe_report: hex::encode(&data.qe_report),
                        signature: hex::encode(data.signature.to_bytes()),
                        qe_authentication_data: hex::encode(&data.qe_authentication_data),
                        certification_data_type: cert_type.to_string(),
                        certification_data: cert_data,
                    })
                }
                TdxCert::PlatformManifest(data) => {
                    CertificationData::PlatformManifest(hex::encode(&data))
                }
                // Return error for unknown certification data variants
                _ => return Err("Unknown CertificationData variant encountered".to_string()),
            },
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeAttestRequest {
    /// Client-provided nonce for freshness (32 bytes as hex string)
    pub nonce: String,
    /// Optional application ID to include in attestation
    /// If provided, the application's bytecode BlobId (hash) will be included in report_data
    pub application_id: Option<ApplicationId>,
}

impl TeeAttestRequest {
    pub fn new(nonce: String, application_id: Option<ApplicationId>) -> Self {
        Self {
            nonce,
            application_id,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeInfoResponseData {
    /// Cloud provider (e.g., "gcp", "azure", "unknown")
    pub cloud_provider: String,
    /// OS image name (e.g., "ubuntu-2404-tdx-v20250115")
    pub os_image: String,
    /// MRTD extracted from TD report (48 bytes hex)
    pub mrtd: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeInfoResponse {
    pub data: TeeInfoResponseData,
}

impl TeeInfoResponse {
    pub fn new(cloud_provider: String, os_image: String, mrtd: String) -> Self {
        Self {
            data: TeeInfoResponseData {
                cloud_provider,
                os_image,
                mrtd,
            },
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeAttestResponseData {
    /// Base64-encoded TDX quote
    /// The quote contains the report_data which the client must verify
    pub quote_b64: String,
    /// Parsed TDX quote structure
    pub quote: Quote,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeAttestResponse {
    pub data: TeeAttestResponseData,
}

impl TeeAttestResponse {
    pub fn new(quote_b64: String, quote: Quote) -> Self {
        Self {
            data: TeeAttestResponseData { quote_b64, quote },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeVerifyQuoteRequest {
    /// Base64-encoded TDX quote to verify
    pub quote_b64: String,
    /// Client-provided nonce that should match report_data[0..32] (64 hex chars = 32 bytes)
    pub nonce: String,
    /// Optional expected application hash that should match report_data[32..64] (64 hex chars = 32 bytes)
    pub expected_application_hash: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeVerifyQuoteResponseData {
    /// Whether the quote signature and certificate chain are valid
    pub quote_verified: bool,
    /// Whether the nonce matches report_data[0..32]
    pub nonce_verified: bool,
    /// Whether the application hash matches report_data[32..64] (if provided)
    pub application_hash_verified: Option<bool>,
    /// Parsed quote structure
    pub quote: Quote,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeVerifyQuoteResponse {
    pub data: TeeVerifyQuoteResponseData,
}

impl TeeVerifyQuoteResponse {
    pub fn new(data: TeeVerifyQuoteResponseData) -> Self {
        Self { data }
    }
}
