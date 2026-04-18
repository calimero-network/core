use std::collections::BTreeMap;

use calimero_context_config::types::{Capability, SignedGroupOpenInvitation};
use calimero_primitives::alias::Alias;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId, GroupMemberRole, UpgradePolicy};
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetLatestVersionResponse {
    pub application_id: Option<ApplicationId>,
    /// Version string of the latest release (e.g. "1.0.0")
    pub version: Option<String>,
}

impl GetLatestVersionResponse {
    pub const fn new(application_id: Option<ApplicationId>, version: Option<String>) -> Self {
        Self {
            application_id,
            version,
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
    pub application_id: ApplicationId,
    /// Which service from the application bundle to run. Optional for single-service apps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_name: Option<String>,
    pub context_seed: Option<Hash>,
    pub initialization_params: Vec<u8>,
    pub group_id: String,
    pub identity_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

impl CreateContextRequest {
    pub const fn new(
        application_id: ApplicationId,
        context_seed: Option<Hash>,
        initialization_params: Vec<u8>,
        group_id: String,
        identity_secret: Option<String>,
    ) -> Self {
        Self {
            application_id,
            service_name: None,
            context_seed,
            initialization_params,
            group_id,
            identity_secret,
            alias: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default)]
    pub group_created: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextResponse {
    pub data: CreateContextResponseData,
}

impl CreateContextResponse {
    pub fn new(context_id: ContextId, member_public_key: PublicKey) -> Self {
        Self {
            data: CreateContextResponseData {
                context_id,
                member_public_key,
                group_id: None,
                group_created: false,
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteContextApiRequest {
    /// Identity of the caller. Required when deleting a group-attached context;
    /// the caller must be a group admin.
    pub requester: Option<PublicKey>,
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
pub struct ContextWithGroup {
    #[serde(flatten)]
    pub context: Context,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextsResponseData {
    pub contexts: Vec<ContextWithGroup>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextsResponse {
    pub data: GetContextsResponseData,
}

impl GetContextsResponse {
    pub const fn new(contexts: Vec<ContextWithGroup>) -> Self {
        Self {
            data: GetContextsResponseData { contexts },
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContextApplicationRequest {
    pub application_id: ApplicationId,
    pub executor_public_key: PublicKey,
    /// Optional migration function name to execute during the update.
    /// The function must be decorated with `#[app::migrate]` in the new application.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migrate_method: Option<String>,
}

impl UpdateContextApplicationRequest {
    pub const fn new(application_id: ApplicationId, executor_public_key: PublicKey) -> Self {
        Self {
            application_id,
            executor_public_key,
            migrate_method: None,
        }
    }

    pub fn with_migration(
        application_id: ApplicationId,
        executor_public_key: PublicKey,
        migrate_method: String,
    ) -> Self {
        Self {
            application_id,
            executor_public_key,
            migrate_method: Some(migrate_method),
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
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum SignatureMetadataEnum {
    NEAR(NearSignatureMessageMetadata),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct NearSignatureMessageMetadata {
    pub recipient: String,
    pub callback_url: String,
    pub nonce: String,
}

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
                            );
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FleetJoinRequest {
    pub group_id: String,
}

impl Validate for FleetJoinRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.group_id.len() != 64 {
            errors.push(ValidationError::InvalidLength {
                field: "group_id",
                expected: 64,
                actual: self.group_id.len(),
            });
        } else if hex::decode(&self.group_id).is_err() {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "group_id",
                reason: "not valid hex".to_owned(),
            });
        }
        errors
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

// -------------------------------------------- Validation Implementations --------------------------------------------
//
// Validation Strategy:
// ====================
// These implementations focus on validating user-controlled string fields and size limits.
//
// Types like `ContextId`, `PublicKey`, and `ApplicationId` are validated during
// serde deserialization - they implement `FromStr` which performs format validation (e.g.,
// base58 decoding, length checks). If deserialization succeeds, the type is guaranteed valid.
//
// For request types containing only these strongly-typed fields, the `Validate` impl returns
// an empty Vec since no additional runtime validation is needed beyond what serde already does.
//
// This approach provides:
// 1. Type-safe validation at the deserialization boundary
// 2. Additional size/format checks for user-provided strings (method names, URLs, etc.)
// 3. Protection against oversized payloads that could cause resource exhaustion

use crate::validation::{
    helpers::{
        validate_bytes_size, validate_hex_string, validate_optional_hex_string,
        validate_optional_string_length, validate_string_length, validate_url,
    },
    Validate, ValidationError, MAX_INIT_PARAMS_SIZE, MAX_METADATA_SIZE, MAX_METHOD_NAME_LENGTH,
    MAX_NONCE_LENGTH, MAX_PACKAGE_NAME_LENGTH, MAX_PATH_LENGTH, MAX_QUOTE_B64_LENGTH,
    MAX_VERSION_LENGTH,
};

impl Validate for InstallApplicationRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if let Some(e) = validate_url(&self.url, "url") {
            errors.push(e);
        }

        if let Some(e) = validate_bytes_size(&self.metadata, "metadata", MAX_METADATA_SIZE) {
            errors.push(e);
        }

        if let Some(e) =
            validate_optional_string_length(&self.package, "package", MAX_PACKAGE_NAME_LENGTH)
        {
            errors.push(e);
        }

        if let Some(e) =
            validate_optional_string_length(&self.version, "version", MAX_VERSION_LENGTH)
        {
            errors.push(e);
        }

        errors
    }
}

impl Validate for InstallDevApplicationRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if self.path.as_str().len() > MAX_PATH_LENGTH {
            errors.push(ValidationError::StringTooLong {
                field: "path",
                max: MAX_PATH_LENGTH,
                actual: self.path.as_str().len(),
            });
        }

        if let Some(e) = validate_bytes_size(&self.metadata, "metadata", MAX_METADATA_SIZE) {
            errors.push(e);
        }

        if let Some(e) =
            validate_optional_string_length(&self.package, "package", MAX_PACKAGE_NAME_LENGTH)
        {
            errors.push(e);
        }

        if let Some(e) =
            validate_optional_string_length(&self.version, "version", MAX_VERSION_LENGTH)
        {
            errors.push(e);
        }

        errors
    }
}

impl Validate for CreateContextRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        if let Some(e) = validate_bytes_size(
            &self.initialization_params,
            "initialization_params",
            MAX_INIT_PARAMS_SIZE,
        ) {
            errors.push(e);
        }

        errors
    }
}

impl Validate for InviteSpecializedNodeRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // All fields are typed (ContextId, Option<PublicKey>) which have their own validation
        Vec::new()
    }
}

impl Validate for UpdateContextApplicationRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Validate migrate_method if provided
        if let Some(ref method) = self.migrate_method {
            if let Some(e) =
                validate_string_length(method, "migrate_method", MAX_METHOD_NAME_LENGTH)
            {
                errors.push(e);
            }

            if method.is_empty() {
                errors.push(ValidationError::EmptyField {
                    field: "migrate_method",
                });
            }
        }

        errors
    }
}

impl Validate for GrantPermissionRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // Note: This is defined in grant_capabilities.rs handler, not here
        // But we still validate the admin.rs version if used
        Vec::new()
    }
}

impl Validate for RevokePermissionRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // All fields are typed which have their own validation
        Vec::new()
    }
}

impl Validate for TeeAttestRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Nonce must be exactly 64 hex characters (32 bytes)
        if let Some(e) = validate_hex_string(&self.nonce, "nonce", 32) {
            errors.push(e);
        }

        errors
    }
}

impl Validate for TeeVerifyQuoteRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Quote base64 size limit
        if self.quote_b64.len() > MAX_QUOTE_B64_LENGTH {
            errors.push(ValidationError::StringTooLong {
                field: "quote_b64",
                max: MAX_QUOTE_B64_LENGTH,
                actual: self.quote_b64.len(),
            });
        }

        // Nonce must be exactly 64 hex characters (32 bytes)
        if let Some(e) = validate_hex_string(&self.nonce, "nonce", 32) {
            errors.push(e);
        }

        // Expected application hash must be exactly 64 hex characters (32 bytes) if provided
        if let Some(e) = validate_optional_hex_string(
            &self.expected_application_hash,
            "expected_application_hash",
            32,
        ) {
            errors.push(e);
        }

        errors
    }
}

impl Validate for AddPublicKeyRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Validate nonce in SignatureMessage
        if let Some(e) = validate_string_length(
            &self.payload.message.nonce,
            "payload.message.nonce",
            MAX_NONCE_LENGTH,
        ) {
            errors.push(e);
        }

        errors
    }
}

impl Validate for JwtTokenRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // executor_public_key should be a reasonable length
        if self.executor_public_key.len() > 128 {
            errors.push(ValidationError::StringTooLong {
                field: "executor_public_key",
                max: 128,
                actual: self.executor_public_key.len(),
            });
        }

        if self.executor_public_key.is_empty() {
            errors.push(ValidationError::EmptyField {
                field: "executor_public_key",
            });
        }

        errors
    }
}

impl Validate for JwtRefreshRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // Refresh tokens are typically JWTs which shouldn't exceed a reasonable size
        if self.refresh_token.len() > 4096 {
            errors.push(ValidationError::StringTooLong {
                field: "refresh_token",
                max: 4096,
                actual: self.refresh_token.len(),
            });
        }

        if self.refresh_token.is_empty() {
            errors.push(ValidationError::EmptyField {
                field: "refresh_token",
            });
        }

        errors
    }
}

// -------------------------------------------- Group API --------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupApiRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_key: Option<String>,
    pub application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_group_id: Option<String>,
}

impl Validate for CreateGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if let Some(ref app_key) = self.app_key {
            if app_key.is_empty() {
                errors.push(ValidationError::EmptyField { field: "app_key" });
            }
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupApiResponse {
    pub data: CreateGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupApiResponseData {
    pub group_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNamespaceApiRequest {
    pub application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

impl Validate for CreateNamespaceApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNamespaceApiResponseData {
    pub namespace_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNamespaceApiResponse {
    pub data: CreateNamespaceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNamespaceApiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for DeleteNamespaceApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNamespaceApiResponseData {
    pub is_deleted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteNamespaceApiResponse {
    pub data: DeleteNamespaceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGroupApiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for DeleteGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGroupApiResponse {
    pub data: DeleteGroupApiResponseData,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteGroupApiResponseData {
    pub is_deleted: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfoApiResponse {
    pub data: GroupInfoApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupInfoApiResponseData {
    pub group_id: String,
    pub app_key: String,
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub member_count: u64,
    pub context_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_upgrade: Option<GroupUpgradeStatusApiData>,
    pub default_capabilities: u32,
    pub default_visibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddGroupMembersApiRequest {
    pub members: Vec<GroupMemberApiInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for AddGroupMembersApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.members.is_empty() {
            errors.push(ValidationError::EmptyField { field: "members" });
        }
        for member in &self.members {
            if member.role == GroupMemberRole::ReadOnlyTee {
                errors.push(ValidationError::InvalidFormat {
                    field: "members[].role",
                    reason: "ReadOnlyTee role can only be assigned via TEE attestation".to_owned(),
                });
            }
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberApiInput {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveGroupMembersApiRequest {
    pub members: Vec<PublicKey>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for RemoveGroupMembersApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.members.is_empty() {
            errors.push(ValidationError::EmptyField { field: "members" });
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListGroupMembersApiResponse {
    pub members: Vec<GroupMemberApiEntry>,
    /// The calling node's own group-level identity (SignerId), so clients
    /// can identify which entry in `members` represents the current user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self_identity: Option<PublicKey>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberApiEntry {
    pub identity: PublicKey,
    pub role: GroupMemberRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListGroupMembersQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupContextEntryResponse {
    pub context_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListGroupContextsApiResponse {
    pub data: Vec<GroupContextEntryResponse>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListGroupContextsQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiRequest {
    pub target_application_id: ApplicationId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub migrate_method: Option<String>,
}

impl Validate for UpgradeGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if let Some(ref method) = self.migrate_method {
            if let Some(e) =
                validate_string_length(method, "migrate_method", MAX_METHOD_NAME_LENGTH)
            {
                errors.push(e);
            }
            if method.is_empty() {
                errors.push(ValidationError::EmptyField {
                    field: "migrate_method",
                });
            }
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiResponse {
    pub data: UpgradeGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeGroupApiResponseData {
    pub group_id: String,
    pub status: String,
    pub total: Option<u32>,
    pub completed: Option<u32>,
    pub failed: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetGroupUpgradeStatusApiResponse {
    pub data: Option<GroupUpgradeStatusApiData>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupUpgradeStatusApiData {
    pub from_version: String,
    pub to_version: String,
    pub initiated_at: u64,
    pub initiated_by: PublicKey,
    pub status: String,
    pub total: Option<u32>,
    pub completed: Option<u32>,
    pub failed: Option<u32>,
    pub completed_at: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetryGroupUpgradeApiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for RetryGroupUpgradeApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
    /// Duration in seconds for the invitation validity.
    /// Defaults to 1 year when not provided.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiration_timestamp: Option<u64>,
    #[serde(default)]
    pub recursive: Option<bool>,
}

impl Validate for CreateGroupInvitationApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiResponse {
    pub data: CreateGroupInvitationApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInvitationApiResponseData {
    pub invitation: SignedGroupOpenInvitation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecursiveInvitationEntry {
    pub group_id: String,
    pub invitation: SignedGroupOpenInvitation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecursiveInvitationApiResponseData {
    pub invitations: Vec<RecursiveInvitationEntry>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRecursiveInvitationApiResponse {
    pub data: CreateRecursiveInvitationApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NestGroupApiRequest {
    pub child_group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for NestGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NestGroupApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnnestGroupApiRequest {
    pub child_group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for UnnestGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnnestGroupApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgroupEntryApiResponse {
    pub group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSubgroupsApiResponse {
    pub subgroups: Vec<SubgroupEntryApiResponse>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceGroupEntryApiResponse {
    pub group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespaceGroupsApiResponse {
    pub data: Vec<NamespaceGroupEntryApiResponse>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiRequest {
    pub invitation: SignedGroupOpenInvitation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_alias: Option<String>,
}

impl Validate for JoinGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiResponse {
    pub data: JoinGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinGroupApiResponseData {
    pub group_id: String,
    pub member_identity: PublicKey,
    pub governance_op: String,
}

// ---- Claim Group Invitation ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimGroupInvitationApiRequest {
    pub governance_op: String,
}

impl Validate for ClaimGroupInvitationApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimGroupInvitationApiResponse {
    pub data: ClaimGroupInvitationApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaimGroupInvitationApiResponseData {
    pub success: bool,
}

// ---- List All Groups ----

#[derive(Clone, Debug, Deserialize)]
pub struct ListAllGroupsQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListAllGroupsApiResponse {
    pub data: Vec<GroupSummaryApiData>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummaryApiData {
    pub group_id: String,
    pub app_key: String,
    pub target_application_id: ApplicationId,
    pub upgrade_policy: UpgradePolicy,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

// ---- Update Group Settings ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateGroupSettingsApiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
    pub upgrade_policy: UpgradePolicy,
}

impl Validate for UpdateGroupSettingsApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

// ---- Update Group Settings ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct UpdateGroupSettingsApiResponse {}

// ---- Update Member Role ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct UpdateMemberRoleApiResponse {}

// ---- Add Group Members (empty response) ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct AddGroupMembersApiResponse {}

// ---- Remove Group Members (empty response) ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct RemoveGroupMembersApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMemberRoleApiRequest {
    pub role: GroupMemberRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for UpdateMemberRoleApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.role == GroupMemberRole::ReadOnlyTee {
            errors.push(ValidationError::InvalidFormat {
                field: "role",
                reason: "ReadOnlyTee role can only be assigned via TEE attestation".to_owned(),
            });
        }
        errors
    }
}

// ---- Detach Context From Group ----

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct DetachContextFromGroupApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetachContextFromGroupApiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for DetachContextFromGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

// ---- Register Group Signing Key ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterGroupSigningKeyApiRequest {
    pub signing_key: String,
}

impl Validate for RegisterGroupSigningKeyApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.signing_key.is_empty() {
            errors.push(ValidationError::EmptyField {
                field: "signing_key",
            });
        }
        errors
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterGroupSigningKeyApiResponse {
    pub data: RegisterGroupSigningKeyApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterGroupSigningKeyApiResponseData {
    pub public_key: PublicKey,
}

// ---- Sync Group ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncGroupApiRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SyncGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncGroupApiResponse {
    pub data: SyncGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncGroupApiResponseData {
    pub group_id: String,
    pub app_key: String,
    pub target_application_id: ApplicationId,
    pub member_count: u64,
    pub context_count: u64,
}

// ---- Join Context ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextApiRequest {
    pub context_id: ContextId,
}

impl Validate for JoinContextApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextApiResponse {
    pub data: JoinContextApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinContextApiResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

// ---- Get Context Group ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetContextGroupApiResponse {
    pub data: Option<String>,
}

// ---- Group Permissions API ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMemberCapabilitiesApiRequest {
    pub capabilities: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetMemberCapabilitiesApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMemberCapabilitiesApiResponse {}

// ---- Set Member Alias ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMemberAliasApiRequest {
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetMemberAliasApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.alias.is_empty() {
            errors.push(ValidationError::EmptyField { field: "alias" });
        }
        if let Some(e) = validate_string_length(&self.alias, "alias", 64) {
            errors.push(e);
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMemberAliasApiResponse {}

// ---- Set Group Alias ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetGroupAliasApiRequest {
    pub alias: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetGroupAliasApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.alias.is_empty() {
            errors.push(ValidationError::EmptyField { field: "alias" });
        }
        if let Some(e) = validate_string_length(&self.alias, "alias", 64) {
            errors.push(e);
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetGroupAliasApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMemberCapabilitiesApiResponse {
    pub data: GetMemberCapabilitiesApiData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMemberCapabilitiesApiData {
    pub capabilities: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultCapabilitiesApiRequest {
    pub default_capabilities: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetDefaultCapabilitiesApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetDefaultCapabilitiesApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetTeeAdmissionPolicyApiRequest {
    #[serde(default)]
    pub allowed_mrtd: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr0: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr1: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr2: Vec<String>,
    #[serde(default)]
    pub allowed_rtmr3: Vec<String>,
    #[serde(default)]
    pub allowed_tcb_statuses: Vec<String>,
    #[serde(default)]
    pub accept_mock: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetTeeAdmissionPolicyApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.allowed_mrtd.is_empty() && !self.accept_mock {
            errors.push(ValidationError::InvalidFormat {
                field: "allowed_mrtd",
                reason: "at least one MRTD must be specified when accept_mock is false".to_owned(),
            });
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetTeeAdmissionPolicyApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetTeeAdmissionPolicyApiResponse {
    pub enabled: bool,
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
    pub allowed_tcb_statuses: Vec<String>,
    pub accept_mock: bool,
}

impl GetTeeAdmissionPolicyApiResponse {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            allowed_mrtd: vec![],
            allowed_rtmr0: vec![],
            allowed_rtmr1: vec![],
            allowed_rtmr2: vec![],
            allowed_rtmr3: vec![],
            allowed_tcb_statuses: vec![],
            accept_mock: false,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDefaultVisibilityApiRequest {
    pub default_visibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetDefaultVisibilityApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.default_visibility != "open" && self.default_visibility != "restricted" {
            errors.push(ValidationError::InvalidFormat {
                field: "default_visibility",
                reason: "must be 'open' or 'restricted'".into(),
            });
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetDefaultVisibilityApiResponse {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_context_response_serializes_with_group_info() {
        let context_id = ContextId::from([0xAA; 32]);
        let member_pk = PublicKey::from([0xBB; 32]);
        let group_id_hex = hex::encode([0xCC; 32]);

        let resp = CreateContextResponseData {
            context_id,
            member_public_key: member_pk,
            group_id: Some(group_id_hex.clone()),
            group_created: true,
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["groupId"], group_id_hex);
        assert_eq!(json["groupCreated"], true);
        assert!(json["contextId"].is_string());
        assert!(json["memberPublicKey"].is_string());
    }

    #[test]
    fn create_context_response_omits_group_id_when_none() {
        let context_id = ContextId::from([0xAA; 32]);
        let member_pk = PublicKey::from([0xBB; 32]);

        let resp = CreateContextResponseData {
            context_id,
            member_public_key: member_pk,
            group_id: None,
            group_created: false,
        };

        let json = serde_json::to_value(&resp).unwrap();
        // groupId should be omitted (skip_serializing_if = "Option::is_none")
        assert!(json.get("groupId").is_none());
        assert_eq!(json["groupCreated"], false);
    }

    #[test]
    fn create_context_response_deserializes_without_group_fields() {
        // Backwards compatibility: old responses without groupId/groupCreated
        // Use valid base58 IDs (ContextId and PublicKey serialize as base58)
        let context_id = ContextId::from([0xAA; 32]);
        let member_pk = PublicKey::from([0xBB; 32]);
        let json = serde_json::json!({
            "contextId": serde_json::to_value(&context_id).unwrap(),
            "memberPublicKey": serde_json::to_value(&member_pk).unwrap()
        });

        let resp: CreateContextResponseData = serde_json::from_value(json).unwrap();
        assert!(resp.group_id.is_none());
        assert!(!resp.group_created);
    }
}

// ---------------------------------------------------------------------------
// Namespace API types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceApiResponse {
    pub namespace_id: String,
    pub app_key: String,
    pub target_application_id: String,
    pub upgrade_policy: String,
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub member_count: usize,
    pub context_count: usize,
    pub subgroup_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNamespaceApiResponse {
    pub data: NamespaceApiResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespacesApiResponse {
    pub data: Vec<NamespaceApiResponse>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceIdentityApiResponse {
    pub namespace_id: String,
    pub public_key: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespacesQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListNamespacesForApplicationQuery {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}
