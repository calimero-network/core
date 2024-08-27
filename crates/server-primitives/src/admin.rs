use calimero_primitives::identity::PublicKey;
use camino::Utf8PathBuf;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct InstallApplicationRequest {
    pub url: url::Url,
    pub version: Option<semver::Version>,
    pub hash: Option<calimero_primitives::hash::Hash>,
    pub metadata: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct InstallDevApplicationRequest {
    pub path: Utf8PathBuf,
    pub version: Option<semver::Version>,
    pub metadata: Vec<u8>,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ApplicationListResult {
    pub apps: Vec<calimero_primitives::application::Application>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListApplicationsResponse {
    pub data: ApplicationListResult,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetApplicationDetailsResponse {
    pub data: calimero_primitives::application::Application,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct InstallApplicationResponse {
    pub data: ApplicationInstallResult,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ApplicationInstallResult {
    pub application_id: calimero_primitives::application::ApplicationId,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetApplicationResponse {
    pub data: GetApplicationResult,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetApplicationResult {
    pub application: Option<calimero_primitives::application::Application>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddPublicKeyRequest {
    pub wallet_signature: String,
    pub payload: Payload,
    pub wallet_metadata: WalletMetadata,
    pub context_id: Option<calimero_primitives::context::ContextId>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    pub message: SignatureMessage,
    pub metadata: SignatureMetadataEnum,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
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
pub struct WalletMetadata {
    #[serde(rename = "wallet")]
    pub wallet_type: calimero_primitives::identity::WalletType,
    pub signing_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum SignatureMetadataEnum {
    NEAR(NearSignatureMessageMetadata),
    ETH(EthSignatureMessageMetadata),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NearSignatureMessageMetadata {
    pub recipient: String,
    pub callback_url: String,
    pub nonce: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EthSignatureMessageMetadata {}

// Intermediate structs for initial parsing
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntermediateAddPublicKeyRequest {
    pub wallet_signature: String,
    pub payload: IntermediatePayload,
    pub wallet_metadata: WalletMetadata, // Reuse WalletMetadata as it fits the intermediate step
    pub context_id: Option<calimero_primitives::context::ContextId>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JwtTokenRequest {
    pub context_id: calimero_primitives::context::ContextId,
    pub executor_public_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JwtRefreshRequest {
    pub refresh_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntermediatePayload {
    pub message: SignatureMessage, // Reuse SignatureMessage as it fits the intermediate step
    pub metadata: Value,           // Raw JSON value for the metadata
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodeChallenge {
    #[serde(flatten)]
    pub message: NodeChallengeMessage,
    pub node_signature: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodeChallengeMessage {
    pub nonce: String,
    pub context_id: Option<calimero_primitives::context::ContextId>,
    pub timestamp: i64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ContextStorage {
    pub size_in_bytes: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextList {
    pub contexts: Vec<calimero_primitives::context::Context>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GetContextsResponse {
    pub data: ContextList,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateContextRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
    pub context_id: Option<calimero_primitives::context::ContextId>,
    pub initialization_params: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ContextResponse {
    pub context: calimero_primitives::context::Context,
    pub member_public_key: PublicKey,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateContextResponse {
    pub data: ContextResponse,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateContextApplicationRequest {
    pub application_id: calimero_primitives::application::ApplicationId,
}
