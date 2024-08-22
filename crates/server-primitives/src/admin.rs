use calimero_primitives::identity::PublicKey;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct InstallApplicationRequest {
    pub url: url::Url,
    pub version: Option<semver::Version>,
    pub hash: Option<calimero_primitives::hash::Hash>,
    pub metadata: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct InstallDevApplicationRequest {
    pub path: Utf8PathBuf,
    pub version: Option<semver::Version>,
    pub metadata: Vec<u8>,
}

impl InstallDevApplicationRequest {
    #[must_use]
    pub const fn new(
        path: Utf8PathBuf,
        version: Option<semver::Version>,
        metadata: Vec<u8>,
    ) -> Self {
        Self {
            path,
            version,
            metadata,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ApplicationListResult {
    pub apps: Vec<calimero_primitives::application::Application>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ListApplicationsResponse {
    pub data: ApplicationListResult,
}

impl ListApplicationsResponse {
    #[must_use]
    pub const fn new(apps: Vec<calimero_primitives::application::Application>) -> Self {
        Self {
            data: ApplicationListResult { apps },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct InstallApplicationResponse {
    pub data: ApplicationInstallResult,
}

impl InstallApplicationResponse {
    #[must_use]
    pub const fn new(application_id: calimero_primitives::application::ApplicationId) -> Self {
        Self {
            data: ApplicationInstallResult { application_id },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ApplicationInstallResult {
    pub application_id: calimero_primitives::application::ApplicationId,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GetApplicationResponse {
    pub data: GetApplicationResult,
}

impl GetApplicationResponse {
    #[must_use]
    pub const fn new(application: Option<calimero_primitives::application::Application>) -> Self {
        Self {
            data: GetApplicationResult { application },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GetApplicationResult {
    pub application: Option<calimero_primitives::application::Application>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct AddPublicKeyRequest {
    pub wallet_signature: String,
    pub payload: Payload,
    pub wallet_metadata: WalletMetadata,
    pub context_id: Option<calimero_primitives::context::ContextId>,
}

impl AddPublicKeyRequest {
    #[must_use]
    pub const fn new(
        wallet_signature: String,
        payload: Payload,
        wallet_metadata: WalletMetadata,
        context_id: Option<calimero_primitives::context::ContextId>,
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
    pub context_id: Option<calimero_primitives::context::ContextId>,
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
    pub wallet_type: calimero_primitives::identity::WalletType,
    pub signing_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
#[non_exhaustive]
pub enum SignatureMetadataEnum {
    NEAR(NearSignatureMessageMetadata),
    ETH(EthSignatureMessageMetadata),
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
pub struct EthSignatureMessageMetadata;

// Intermediate structs for initial parsing
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct IntermediateAddPublicKeyRequest {
    pub wallet_signature: String,
    pub payload: IntermediatePayload,
    pub wallet_metadata: WalletMetadata, // Reuse WalletMetadata as it fits the intermediate step
    pub context_id: Option<calimero_primitives::context::ContextId>,
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
    pub context_id: Option<calimero_primitives::context::ContextId>,
    pub timestamp: i64,
}

impl NodeChallengeMessage {
    #[must_use]
    pub const fn new(
        nonce: String,
        context_id: Option<calimero_primitives::context::ContextId>,
        timestamp: i64,
    ) -> Self {
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
    pub contexts: Vec<calimero_primitives::context::Context>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct GetContextsResponse {
    pub data: ContextList,
}

impl GetContextsResponse {
    #[must_use]
    pub const fn new(contexts: Vec<calimero_primitives::context::Context>) -> Self {
        Self {
            data: ContextList { contexts },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct CreateContextRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
}

impl CreateContextRequest {
    #[must_use]
    pub const fn new(application_id: calimero_primitives::application::ApplicationId) -> Self {
        Self { application_id }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ContextResponse {
    pub context: calimero_primitives::context::Context,
    pub member_public_key: PublicKey,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct CreateContextResponse {
    pub data: ContextResponse,
}

impl CreateContextResponse {
    #[must_use]
    pub const fn new(
        context: calimero_primitives::context::Context,
        member_public_key: PublicKey,
    ) -> Self {
        Self {
            data: ContextResponse {
                context,
                member_public_key,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct UpdateContextApplicationRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
}

impl UpdateContextApplicationRequest {
    #[must_use]
    pub const fn new(application_id: calimero_primitives::application::ApplicationId) -> Self {
        Self { application_id }
    }
}
