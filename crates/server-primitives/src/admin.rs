use calimero_primitives::application::{Application, ApplicationId};
use calimero_primitives::context::{Context, ContextId, ContextInvitationPayload};
use calimero_primitives::hash::Hash;
use calimero_primitives::identity::{PrivateKey, PublicKey, WalletType};
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct InstallApplicationRequest {
    pub url: Url,
    pub hash: Option<Hash>,
    pub metadata: Vec<u8>,
}

impl InstallApplicationRequest {
    #[must_use]
    pub const fn new(url: Url, hash: Option<Hash>, metadata: Vec<u8>) -> Self {
        Self {
            url,
            hash,
            metadata,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct InstallDevApplicationRequest {
    pub path: Utf8PathBuf,
    pub metadata: Vec<u8>,
}

impl InstallDevApplicationRequest {
    #[must_use]
    pub const fn new(path: Utf8PathBuf, metadata: Vec<u8>) -> Self {
        Self { path, metadata }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ApplicationListResult {
    pub apps: Vec<Application>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ListApplicationsResponse {
    pub data: ApplicationListResult,
}

impl ListApplicationsResponse {
    #[must_use]
    pub const fn new(apps: Vec<Application>) -> Self {
        Self {
            data: ApplicationListResult { apps },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GetApplicationDetailsResponse {
    pub data: Application,
}

impl GetApplicationDetailsResponse {
    #[must_use]
    pub const fn new(application: Application) -> Self {
        Self { data: application }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct InstallApplicationResponse {
    pub data: ApplicationInstallResult,
}

impl InstallApplicationResponse {
    #[must_use]
    pub const fn new(application_id: ApplicationId) -> Self {
        Self {
            data: ApplicationInstallResult { application_id },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ApplicationInstallResult {
    pub application_id: ApplicationId,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GetApplicationResponse {
    pub data: GetApplicationResult,
}

impl GetApplicationResponse {
    #[must_use]
    pub const fn new(application: Option<Application>) -> Self {
        Self {
            data: GetApplicationResult { application },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GetApplicationResult {
    pub application: Option<Application>,
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
#[allow(clippy::exhaustive_structs)]
#[allow(clippy::empty_structs_with_brackets)]
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
#[non_exhaustive]
pub struct StarknetPayload {
    pub signature: Vec<String>,
    #[serde(rename = "messageHash")]
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

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ContextStorage {
    pub size_in_bytes: u64,
}

impl ContextStorage {
    #[must_use]
    pub const fn new(size_in_bytes: u64) -> Self {
        Self { size_in_bytes }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ContextList {
    pub contexts: Vec<Context>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GetContextsResponse {
    pub data: ContextList,
}

impl GetContextsResponse {
    #[must_use]
    pub const fn new(contexts: Vec<Context>) -> Self {
        Self {
            data: ContextList { contexts },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CreateContextRequest {
    pub application_id: ApplicationId,
    pub context_seed: Option<Hash>,
    pub initialization_params: Vec<u8>,
}

impl CreateContextRequest {
    #[must_use]
    pub const fn new(
        application_id: ApplicationId,
        context_seed: Option<Hash>,
        initialization_params: Vec<u8>,
    ) -> Self {
        Self {
            application_id,
            context_seed,
            initialization_params,
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ContextResponse {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CreateContextResponse {
    pub data: ContextResponse,
}

impl CreateContextResponse {
    #[must_use]
    pub const fn new(context_id: ContextId, member_public_key: PublicKey) -> Self {
        Self {
            data: ContextResponse {
                context_id,
                member_public_key,
            },
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JoinContextRequest {
    pub private_key: PrivateKey,
    pub invitation_payload: ContextInvitationPayload,
}

impl JoinContextRequest {
    #[must_use]
    pub const fn new(
        private_key: PrivateKey,
        invitation_payload: ContextInvitationPayload,
    ) -> Self {
        Self {
            private_key,
            invitation_payload,
        }
    }
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct JoinContextResponseData {
    pub context_id: ContextId,
    pub member_public_key: PublicKey,
}

impl JoinContextResponseData {
    #[must_use]
    pub const fn new(context_id: ContextId, member_public_key: PublicKey) -> Self {
        Self {
            context_id,
            member_public_key,
        }
    }
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize)]
#[non_exhaustive]
pub struct JoinContextResponse {
    pub data: Option<JoinContextResponseData>,
}

impl JoinContextResponse {
    #[must_use]
    pub const fn new(data: Option<JoinContextResponseData>) -> Self {
        Self { data }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UpdateContextApplicationRequest {
    pub application_id: ApplicationId,
}

impl UpdateContextApplicationRequest {
    #[must_use]
    pub const fn new(application_id: ApplicationId) -> Self {
        Self { application_id }
    }
}
