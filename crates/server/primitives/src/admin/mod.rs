use std::collections::BTreeMap;

use calimero_context_config::types::{Capability, SignedGroupOpenInvitation};
use calimero_primitives::alias::Alias;
use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId, GroupMemberRole, UpgradePolicy};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{ClientKey, ContextUser, PublicKey, WalletType};
use calimero_primitives::metadata::MetadataRecord;
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
    pub name: Option<String>,
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
            name: None,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FleetJoinResponse {
    pub status: String,
    pub group_id: String,
    pub namespace_id: String,
    pub public_key: String,
    pub admitted: bool,
    /// `true` if the node successfully published `MemberSetAutoFollow` for
    /// itself after admission. `false` means admission succeeded but the
    /// node will NOT auto-join future contexts until the op is retried.
    #[serde(default)]
    pub auto_follow_enabled: bool,
    pub contexts_joined: Vec<String>,
}

/// Per-column on-disk byte estimates for a namespace.
///
/// Values are RocksDB approximations (`get_approximate_sizes_cf`) — sampled
/// from SST metadata, not exact. Sufficient for quota enforcement, not for
/// audit-level accounting. `total` is the sum of the individual column
/// fields; callers can use it directly rather than re-summing.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceUsageBytes {
    pub state: u64,
    pub private_state: u64,
    pub delta: u64,
    pub governance: u64,
    pub total: u64,
}

/// Per-namespace resource usage on this node.
/// Returned by `GET /admin-api/usage` in the `namespaces` list.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NamespaceUsage {
    pub namespace_id: String,
    pub context_count: u32,
    pub member_count: u32,
    pub subgroup_count: u32,
    pub bytes: NamespaceUsageBytes,
}

/// Response for `GET /admin-api/usage`. Reports per-namespace counts + byte
/// breakdown for every namespace this node participates in. Used by MDMA
/// to enforce plan limits (e.g. 1 GB free tier) and for operator dashboards.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageResponse {
    pub namespaces: Vec<NamespaceUsage>,
}

/// Response for `GET /admin-api/network/status`. Wire-format snapshot of
/// the local node's libp2p connectivity state — what relays we hold
/// reservations with, which rendezvous registrations are live, the
/// outcome of the latest DCUtR hole-punch per peer, and the most recent
/// AutoNAT v2 probe. Surfaced verbatim by `meroctl network status`.
///
/// All multiaddrs / peer ids are stringified, all timestamps are RFC3339
/// UTC, all status fields are flat strings (lowercase enum names). This
/// keeps the wire shape stable across libp2p upgrades and friendly to
/// consumers in any language.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatusResponse {
    pub local_peer_id: String,
    pub listen_addrs: Vec<String>,
    pub external_addrs: Vec<String>,
    pub relays: Vec<RelayStatusEntry>,
    pub rendezvous: Vec<RendezvousStatusEntry>,
    pub direct_upgrades: Vec<DirectUpgradeStatusEntry>,
    pub autonat: AutonatStatusEntry,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayStatusEntry {
    pub peer_id: String,
    /// One of: `discovered`, `requested`, `accepted`, `expired`.
    pub reservation_status: String,
    pub last_state_change: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RendezvousStatusEntry {
    pub peer_id: String,
    /// One of: `discovered`, `requested`, `registered`, `expired`.
    pub registration_status: String,
    pub last_state_change: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirectUpgradeStatusEntry {
    pub peer_id: String,
    /// `succeeded` or `failed`. When `failed`, `reason` is populated.
    pub status: String,
    pub reason: Option<String>,
    pub connection_id: Option<String>,
    pub last_attempt: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AutonatStatusEntry {
    /// One of: `unknown`, `public`, `private`.
    pub reachability: String,
    pub last_test_addr: Option<String>,
    /// `reachable`, `failed`, or `null` if no probe has landed.
    pub last_test_result: Option<String>,
    pub last_test_reason: Option<String>,
    pub last_test_observed_addr: Option<String>,
    pub last_test_at: Option<String>,
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
    pub name: Option<String>,
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
    pub name: Option<String>,
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
    pub subgroup_visibility: String,
    /// Full metadata record for the group (name + opaque `data` map), or
    /// omitted if none has ever been set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MetadataRecord>,
    /// Hex-encoded SHA-256 hash of the group's authorization-relevant
    /// state. Mirrors `contextStateHash` on context responses; lets
    /// clients poll for governance convergence across nodes.
    // Explicit rename pins the JSON name even if the Rust field is
    // refactored, matching the same pattern as `contextStateHash`.
    #[serde(rename = "groupStateHash")]
    pub group_state_hash: String,
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
    pub name: Option<String>,
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
    pub name: Option<String>,
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
    /// When `true`, the handler emits the single atomic `GroupOp::CascadeUpgrade`
    /// op (target + app_key + migration + fence `cascade_hlc`) and dispatches the
    /// per-context migration propagator against every descendant subgroup whose
    /// current `app_key` matches the signed group's current `app_key`.
    ///
    /// Default: `false` — existing clients (e.g. PR-1's single-group
    /// workflow 00) stay on the per-group path bit-identically.
    #[serde(default)]
    pub cascade: bool,
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

/// Per-group cascade migration status entry returned by `get_cascade_status`.
///
/// Mirrors [`GroupUpgradeStatusApiData`] for the upgrade snapshot, augmented
/// with `group_id` and the sticky `cascade_hlc` fence from the atomic
/// `CascadeUpgrade` op (opaque display string; `None` for non-cascade upgrades).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CascadeStatusApiEntry {
    /// Hex-encoded 32-byte group id.
    pub group_id: String,
    /// Upgrade snapshot for this group.
    pub upgrade: GroupUpgradeStatusApiData,
    /// HLC fence string from the atomic `CascadeUpgrade` op, or `null`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cascade_hlc: Option<String>,
}

/// Response returned by `GET .../groups/:namespace_id/cascade-status`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetCascadeStatusApiResponse {
    pub data: Vec<CascadeStatusApiEntry>,
}

/// The freshest reported facts for a pinned-cohort member, surfaced by
/// `get_migration_status` (Task 6c.10). `null` for a member with no fresh
/// heartbeat (its `state` is then `"unknown"`).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberMigrationReportApiData {
    pub schema_version: u32,
    pub residue_auto: u64,
    pub residue_identity: u64,
    pub synced_up_to_hlc: u64,
    pub reported_at: u64,
}

/// One per-member row in the migration-status rollup: a pinned-cohort member,
/// its reported facts (if any), and the derived migration state.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberMigrationStatusApiEntry {
    /// The cohort member.
    pub peer: PublicKey,
    /// The member's freshest reported facts, or `null` when it has no fresh
    /// heartbeat (in which case `state == "unknown"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<MemberMigrationReportApiData>,
    /// Derived state discriminant: `"migrated"`, `"in_progress"`, or `"unknown"`.
    pub state: String,
}

/// Rollup counters across the pinned cohort (observability only — never a gate).
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationStatusRollupApiData {
    pub migrated: usize,
    pub in_progress: usize,
    pub unknown: usize,
    pub total: usize,
    /// `true` iff every pinned-cohort member reported a converged schema with
    /// zero residue. Any `unknown` (or in-progress) member keeps this `false`.
    pub all_migrated: bool,
}

/// Migration-status answer returned by `GET .../groups/:namespace_id/migration-status`.
///
/// The operator-facing "have all peers migrated?" rollup (Task 6c.10): the
/// pinned-cohort size, the per-member rows, and the `all_migrated` flag.
/// Observability only — this endpoint never gates a write or apply.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMigrationStatusApiResponse {
    pub target_version: u32,
    /// Size of the pinned cohort (the inherited-membership closure, minus any
    /// member excluded by the expand-entry HLC pin).
    pub expected_members: usize,
    /// The governance HLC the cohort was pinned at, as an opaque display string;
    /// `null` when there is no migration record.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cohort_pinned_at_hlc: Option<String>,
    pub rollup: MigrationStatusRollupApiData,
    pub members: Vec<MemberMigrationStatusApiEntry>,
}

/// Response returned by `POST .../groups/:namespace_id/migration/abort`.
///
/// Reports whether a pending migration was found and logically aborted. The
/// abort flips the group's migration target back to the pre-migration
/// application and drops the pending migration marker so not-yet-applied lazy
/// contexts stop migrating. It does not recall an already-committed v2 context.
/// Idempotent: aborting with nothing pending returns `aborted: false`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AbortMigrationApiResponse {
    /// Hex-encoded 32-byte namespace id the abort targeted.
    pub namespace_id: String,
    /// `true` when a pending migration was flipped back; `false` for the
    /// idempotent no-op (nothing was pending).
    pub aborted: bool,
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
    pub group_name: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecursiveInvitationEntry {
    pub group_id: String,
    pub invitation: SignedGroupOpenInvitation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_name: Option<String>,
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

/// Atomically move a group to a new parent. Replaces the old
/// nest/unnest pair — orphan state is no longer reachable.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReparentGroupApiRequest {
    pub new_parent_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for ReparentGroupApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.new_parent_id.len() != 64 {
            errors.push(ValidationError::InvalidLength {
                field: "new_parent_id",
                expected: 64,
                actual: self.new_parent_id.len(),
            });
        } else if hex::decode(&self.new_parent_id).is_err() {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "new_parent_id",
                reason: "not valid hex".to_owned(),
            });
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReparentGroupApiResponse {
    pub reparented: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SubgroupEntryApiResponse {
    pub group_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    pub name: Option<String>,
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
    pub group_name: Option<String>,
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
    pub name: Option<String>,
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

// ---- Join Subgroup via Inheritance ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinSubgroupInheritanceApiResponse {
    pub data: JoinSubgroupInheritanceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinSubgroupInheritanceApiResponseData {
    pub group_id: String,
    pub member_public_key: PublicKey,
    pub was_inherited: bool,
}

// ---- Leave Context (local-only opt-out) ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveContextApiResponse {
    pub data: LeaveContextApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveContextApiResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

// ---- Leave Group (distributed self-leave op) ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveGroupApiResponse {
    pub data: LeaveGroupApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveGroupApiResponseData {
    pub group_id: String,
    pub member_public_key: PublicKey,
}

// ---- Issue Ownership Proof ----
//
// Wire format is locked: see github.com/calimero-network/tauri-app#73.
// mdma and tauri-app are implemented separately against this exact shape.
//
// Request: { audience, context_id, subject, nonce, expires_at_ms }
// Response: { signer_public_key, signed_payload, signature }
//
// `signed_payload` is opaque base64-encoded UTF-8 JSON bytes — the verifier
// re-parses them. The signature input is
//   `OWNERSHIP_PROOF_DOMAIN || signed_payload_bytes`
// where `OWNERSHIP_PROOF_DOMAIN` is the 28-byte literal
// `b"calimero.ownership-claim.v1\x00"` (defined in calimero-context).

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueOwnershipProofApiRequest {
    pub audience: String,
    /// Base58 or hex-encoded 32-byte context id. Parsed server-side via
    /// `parse_context_id`.
    pub context_id: String,
    pub subject: String,
    /// Hex string, 32–128 chars inclusive (16–64 raw bytes).
    pub nonce: String,
    /// Caller-requested expiry in unix milliseconds. Server clamps to
    /// `min(expires_at_ms, issued_at_ms + 5*60*1000)`.
    pub expires_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueOwnershipProofApiResponse {
    /// Base58-encoded 32-byte ed25519 public key of the signer.
    pub signer_public_key: String,
    /// Base64-encoded opaque UTF-8 JSON bytes of the canonical claim payload.
    /// Verifiers MUST re-parse this exact byte slice and re-derive the
    /// signature input as `OWNERSHIP_PROOF_DOMAIN || signed_payload_bytes`.
    pub signed_payload: String,
    /// Base64-encoded 64-byte ed25519 signature over the signature input.
    pub signature: String,
}

impl Validate for IssueOwnershipProofApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // audience: non-empty, <= 256 chars.
        if self.audience.is_empty() {
            errors.push(ValidationError::EmptyField { field: "audience" });
        } else if let Some(e) = validate_string_length(&self.audience, "audience", 256) {
            errors.push(e);
        }

        // subject: non-empty, <= 512 chars.
        if self.subject.is_empty() {
            errors.push(ValidationError::EmptyField { field: "subject" });
        } else if let Some(e) = validate_string_length(&self.subject, "subject", 512) {
            errors.push(e);
        }

        // nonce: hex string, 32..=128 chars inclusive (16..=64 raw bytes).
        let n = self.nonce.len();
        if !(32..=128).contains(&n) {
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce must be hex-encoded, 32..=128 characters".into(),
            });
        } else if !self.nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "nonce",
                reason: "nonce must be valid hex".into(),
            });
        } else if n % 2 != 0 {
            // An odd-length hex string can't decode to whole bytes, which is
            // inconsistent with the documented "16..=64 raw bytes" contract.
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce hex string must have even length".into(),
            });
        }

        // context_id and expires_at_ms are validated in the handler (the former
        // because parsing is shared with `parse_context_id`, the latter because
        // it requires comparing against the current wall-clock).

        errors
    }
}

/// Namespace-scoped sibling of [`IssueOwnershipProofApiRequest`].
///
/// Wire-identical MINUS the `contextId` field: the proof is scoped to the
/// namespace-root group only. The response reuses
/// [`IssueOwnershipProofApiResponse`] verbatim. Purely additive to
/// `calimero-server-primitives`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueNamespaceOwnershipProofApiRequest {
    pub audience: String,
    pub subject: String,
    /// Hex string, 32–128 chars inclusive (16–64 raw bytes).
    pub nonce: String,
    /// Caller-requested expiry in unix milliseconds. Server clamps to
    /// `min(expires_at_ms, issued_at_ms + 5*60*1000)`.
    pub expires_at_ms: u64,
}

impl Validate for IssueNamespaceOwnershipProofApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();

        // audience: non-empty, <= 256 chars.
        if self.audience.is_empty() {
            errors.push(ValidationError::EmptyField { field: "audience" });
        } else if let Some(e) = validate_string_length(&self.audience, "audience", 256) {
            errors.push(e);
        }

        // subject: non-empty, <= 512 chars.
        if self.subject.is_empty() {
            errors.push(ValidationError::EmptyField { field: "subject" });
        } else if let Some(e) = validate_string_length(&self.subject, "subject", 512) {
            errors.push(e);
        }

        // nonce: hex string, 32..=128 chars inclusive (16..=64 raw bytes).
        let n = self.nonce.len();
        if !(32..=128).contains(&n) {
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce must be hex-encoded, 32..=128 characters".into(),
            });
        } else if !self.nonce.chars().all(|c| c.is_ascii_hexdigit()) {
            errors.push(ValidationError::InvalidHexEncoding {
                field: "nonce",
                reason: "nonce must be valid hex".into(),
            });
        } else if n % 2 != 0 {
            // An odd-length hex string can't decode to whole bytes, which is
            // inconsistent with the documented "16..=64 raw bytes" contract.
            errors.push(ValidationError::InvalidFormat {
                field: "nonce",
                reason: "nonce hex string must have even length".into(),
            });
        }

        // expires_at_ms is validated in the handler (it requires comparing
        // against the current wall-clock).

        errors
    }
}

// ---- Leave Namespace (cascading self-leave) ----

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveNamespaceApiResponse {
    pub data: LeaveNamespaceApiResponseData,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeaveNamespaceApiResponseData {
    pub namespace_id: String,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMemberAutoFollowApiRequest {
    /// When true, the target auto-joins new contexts registered in this group.
    pub auto_follow_contexts: bool,
    /// When true, the target self-admits into subgroups nested under this group.
    pub auto_follow_subgroups: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetMemberAutoFollowApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        Vec::new()
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMemberAutoFollowApiResponse {}

// ---- Set Metadata (group / member / context) ----

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetMetadataApiRequest {
    /// New display name. Absent field keeps the current name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Replacement opaque `data` map; stored verbatim by core.
    #[serde(default)]
    pub data: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetMetadataApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        // Delegate to the single source of truth — the exact same checks the
        // `*MetadataSet` op-apply path enforces (size limits, non-empty name,
        // non-empty data keys) — so an HTTP request that would later fail at
        // apply time is rejected here with a clean 400 instead.
        match calimero_primitives::metadata::validate_metadata_payload(
            self.name.as_deref(),
            &self.data,
        ) {
            Ok(()) => Vec::new(),
            Err(reason) => vec![ValidationError::InvalidFormat {
                field: "metadata",
                reason,
            }],
        }
    }
}

pub type SetMemberMetadataApiRequest = SetMetadataApiRequest;
pub type SetGroupMetadataApiRequest = SetMetadataApiRequest;
pub type SetContextMetadataApiRequest = SetMetadataApiRequest;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetMetadataApiResponse {}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMetadataApiResponse {
    /// The metadata record, or `null` if none has ever been set for the
    /// target (group / member / context).
    pub data: Option<MetadataRecord>,
}

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
pub struct SetSubgroupVisibilityApiRequest {
    pub subgroup_visibility: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requester: Option<PublicKey>,
}

impl Validate for SetSubgroupVisibilityApiRequest {
    fn validate(&self) -> Vec<ValidationError> {
        let mut errors = Vec::new();
        if self.subgroup_visibility != "open" && self.subgroup_visibility != "restricted" {
            errors.push(ValidationError::InvalidFormat {
                field: "subgroup_visibility",
                reason: "must be 'open' or 'restricted'".into(),
            });
        }
        errors
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SetSubgroupVisibilityApiResponse {}

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

    #[test]
    fn migration_status_response_serializes_rollup_and_members() {
        // The `get_migration_status` admin route (Task 6c.10) returns this
        // shape. Pin the JSON contract: camelCase keys, the per-member `state`
        // discriminant, the `allMigrated` rollup flag, and a `null`-report
        // member surfacing as `unknown` with its `report` field omitted.
        let migrated_peer = PublicKey::from([0x11; 32]);
        let unknown_peer = PublicKey::from([0x22; 32]);

        let resp = GetMigrationStatusApiResponse {
            target_version: 2,
            expected_members: 2,
            cohort_pinned_at_hlc: Some("hlc-abc".into()),
            rollup: MigrationStatusRollupApiData {
                migrated: 1,
                in_progress: 0,
                unknown: 1,
                total: 2,
                all_migrated: false,
            },
            members: vec![
                MemberMigrationStatusApiEntry {
                    peer: migrated_peer,
                    report: Some(MemberMigrationReportApiData {
                        schema_version: 2,
                        residue_auto: 0,
                        residue_identity: 0,
                        synced_up_to_hlc: 7,
                        reported_at: 1_700_000_000,
                    }),
                    state: "migrated".into(),
                },
                MemberMigrationStatusApiEntry {
                    peer: unknown_peer,
                    report: None,
                    state: "unknown".into(),
                },
            ],
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["targetVersion"], 2);
        assert_eq!(json["expectedMembers"], 2);
        assert_eq!(json["cohortPinnedAtHlc"], "hlc-abc");
        assert_eq!(json["rollup"]["allMigrated"], false);
        assert_eq!(json["rollup"]["migrated"], 1);
        assert_eq!(json["rollup"]["unknown"], 1);

        let members = json["members"].as_array().unwrap();
        assert_eq!(members.len(), 2);
        assert_eq!(members[0]["state"], "migrated");
        assert_eq!(members[0]["report"]["schemaVersion"], 2);
        assert_eq!(members[0]["report"]["syncedUpToHlc"], 7);
        // The unknown member has no fresh report — `report` is omitted.
        assert_eq!(members[1]["state"], "unknown");
        assert!(members[1].get("report").is_none());
    }

    #[test]
    fn migration_status_response_omits_hlc_when_absent() {
        // No migration record → `cohortPinnedAtHlc` is omitted.
        let resp = GetMigrationStatusApiResponse {
            target_version: 0,
            expected_members: 0,
            cohort_pinned_at_hlc: None,
            rollup: MigrationStatusRollupApiData {
                migrated: 0,
                in_progress: 0,
                unknown: 0,
                total: 0,
                all_migrated: false,
            },
            members: vec![],
        };

        let json = serde_json::to_value(&resp).unwrap();
        assert!(json.get("cohortPinnedAtHlc").is_none());
        assert_eq!(json["rollup"]["allMigrated"], false);
    }

    fn ownership_req(nonce: &str) -> IssueOwnershipProofApiRequest {
        IssueOwnershipProofApiRequest {
            audience: "mdma.cloud".into(),
            context_id: "11111111111111111111111111111111".into(),
            subject: "subject-xyz".into(),
            nonce: nonce.into(),
            expires_at_ms: 1,
        }
    }

    #[test]
    fn ownership_proof_even_length_hex_nonce_passes() {
        // 32 hex chars (16 bytes) — minimum valid, even length.
        let errors = ownership_req("deadbeefcafebabe1122334455667788").validate();
        assert!(
            errors.is_empty(),
            "even-length hex nonce must validate cleanly, got {errors:?}"
        );
    }

    #[test]
    fn ownership_proof_odd_length_hex_nonce_rejected() {
        // 33 hex chars: in range, all hex digits, but odd length.
        let errors = ownership_req("deadbeefcafebabe1122334455667788a").validate();
        assert!(
            errors.iter().any(|e| matches!(
                e,
                ValidationError::InvalidFormat { field: "nonce", reason }
                    if reason == "nonce hex string must have even length"
            )),
            "odd-length hex nonce must be rejected, got {errors:?}"
        );
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
    pub name: Option<String>,
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
