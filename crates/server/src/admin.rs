use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_identity::auth::verify_eth_signature;
use chrono::{Duration, TimeZone, Utc};
use eyre::bail;
use near_jsonrpc_client::{methods, JsonRpcClient};
use near_jsonrpc_primitives::types::query::QueryResponseKind;
use near_primitives::types::{BlockReference, Finality, FunctionArgs};
use near_primitives::views::QueryRequest;
use rand::{thread_rng, RngCore};
use serde::{Deserialize, Serialize};
use serde_json::{from_slice, json, Value};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use tracing::{error, info};

use crate::verifysignature::{self, verify_signature};

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub(crate) fn service(
    config: &crate::config::ServerConfig,
) -> eyre::Result<Option<(&'static str, Router)>> {
    let _config = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin api is disabled");
            return Ok(None);
        }
    };
    let admin_path = "/admin-api";

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);
    let admin_router = Router::new()
        .route("/health", get(health_check_handler))
        .route("/root-key", post(create_root_key_handler))
        .route("/request-challenge", post(request_challenge_handler))
        .route("/install-application", post(install_application_handler))
        .route("/add-client-key", post(add_client_key_handler))
        .layer(session_layer);

    Ok(Some((admin_path, admin_router)))
}

pub(crate) fn site(
    config: &crate::config::ServerConfig,
) -> eyre::Result<Option<(&'static str, ServeDir<SetStatus<ServeFile>>)>> {
    let _config = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin site is disabled");
            return Ok(None);
        }
    };
    let path = "/admin";

    let react_static_files_path = "./node-ui/dist";
    let react_app_serve_dir = ServeDir::new(react_static_files_path).not_found_service(
        ServeFile::new(format!("{}/index.html", react_static_files_path)),
    );

    Ok(Some((path, react_app_serve_dir)))
}

pub const CHALLENGE_KEY: &str = "challenge";

struct ApiResponse<T: Serialize> {
    payload: T,
}
impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        let body = serde_json::to_string(&self.payload).unwrap();
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    }
}

#[derive(Serialize)]
struct RequestChallengeBody {
    challenge: String,
}

pub async fn request_challenge_handler(session: Session) -> impl IntoResponse {
    if let Some(challenge) = session.get::<String>(CHALLENGE_KEY).await.ok().flatten() {
        ApiResponse {
            payload: RequestChallengeBody { challenge },
        }
        .into_response()
    } else {
        let challenge = generate_challenge();

        if let Err(err) = session.insert(CHALLENGE_KEY, &challenge).await {
            error!("Failed to insert challenge into session: {}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to insert challenge into session",
            )
                .into_response();
        }
        ApiResponse {
            payload: RequestChallengeBody { challenge },
        }
        .into_response()
    }
}

fn generate_random_bytes() -> [u8; 32] {
    let mut rng = thread_rng();
    let mut buf = [0u8; 32];
    rng.fill_bytes(&mut buf);
    buf
}

fn generate_challenge() -> String {
    let random_bytes = generate_random_bytes();
    let encoded = STANDARD.encode(&random_bytes);
    encoded
}

async fn health_check_handler() -> impl IntoResponse {
    (StatusCode::OK, "alive")
}

async fn create_root_key_handler(
    session: Session,
    Json(req): Json<PubKeyRequest>,
) -> impl IntoResponse {
    let message = "helloworld";
    let app = "me";

    match session.get::<String>(CHALLENGE_KEY).await.ok().flatten() {
        Some(challenge) => {
            if verifysignature::verify_signature(
                &challenge,
                message,
                app,
                &req.callback_url,
                &req.signature,
                &req.public_key,
            ) {
                (StatusCode::OK, "Root key created")
            } else {
                (StatusCode::BAD_REQUEST, "Invalid signature")
            }
        }
        _ => (StatusCode::BAD_REQUEST, "Challenge not found"),
    }
}

//* Register client key to authenticate client requests  */
async fn add_client_key_handler(
    _session: Session,
    Json(intermediate_req): Json<IntermediateAddClientKeyRequest>,
) -> impl IntoResponse {
    let req: Result<AddClientKeyRequest, (StatusCode, &str)> = transform_request(intermediate_req)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid payload"));

    if let Err(err) = req {
        return err;
    }

    let req = req.unwrap();
    info!("Request: {:?}", req);

    if let Err(err) = validate_challenge(req.wallet_metadata, &req.wallet_signature, &req.payload) {
        info!("Error with challenge: {:?}", err.to_string());
        return (StatusCode::BAD_REQUEST, "Invalid challenge!");
    }

    // Extract clientPublicKey and add it to list of client keys
    if let Err(err) = store_client_key(&req.payload.message.client_public_key) {
        info!("Error with storing client key: {:?}", err.to_string());
        return (StatusCode::BAD_REQUEST, "Issue while storing client key");
    }

    (StatusCode::OK, "\"data\":\"ok\"")
}

fn check_node_signature(
    wallet_metadata: WalletMetadata,
    wallet_signature: &str,
    payload: &Payload,
) -> eyre::Result<bool> {
    validate_root_key_exists(&wallet_metadata)?;

    validate_challenge_content(&payload)?;

    match wallet_metadata.wallet_type {
        WalletType::NEAR => {
            let near_metadata: &NearSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::NEAR(metadata) => metadata,
                _ => eyre::bail!("Invalid metadata"),
            };

            //Check node signature to make sure that challenge was signed with node root key
            info!(payload.message.node_signature);

            let result = verify_signature(
                &payload.message.nonce,
                &payload.message.message,
                &near_metadata.recipient,
                &near_metadata.callback_url,
                &wallet_signature,
                &wallet_metadata.signing_key,
            );

            info!("NEAR login verify_signature result: {:?}", result);
            if !result {
                eyre::bail!("Node signature is invalid. Please check the signature.")
            }
            Ok(true)
        }
        WalletType::ETH => {
            let _eth_metadata: &EthSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::ETH(metadata) => metadata,
                _ => eyre::bail!("Invalid metadata"),
            };

            let result = verify_eth_signature(
                &wallet_metadata.signing_key,
                &payload.message.message,
                wallet_signature,
            )?;

            info!("ETH login verify_eth_signature result: {:?}", result);
            Ok(true)
        }
    }
}

//check if signature data are not tempered with
fn validate_challenge_content(payload: &Payload) -> eyre::Result<bool> {
    if payload.message.node_signature
        != create_node_signature(
            &payload.message.nonce,
            &payload.message.application_id,
            &payload.message.timestamp,
        )
    {
        eyre::bail!("Node signature is invalid")
    }
    Ok(true)
}

fn create_node_signature(_nonce: &String, _application_id: &String, _timestamp: &i64) -> String {
    //TODO implement node signature
    // get first root key and sign the challenge

    return "abcdefhgjsdajbadk".to_string();
}

//Check if challenge is valid
fn validate_challenge(
    wallet_metadata: WalletMetadata,
    wallet_signature: &str,
    payload: &Payload,
) -> eyre::Result<bool> {
    // Check if node has created signature
    check_node_signature(wallet_metadata, &wallet_signature, &payload)?;

    // Check challenge to verify if it has expired or not
    if is_older_than_15_minutes(payload.message.timestamp) {
        eyre::bail!("Challenge is too old. Please request a new challenge.")
    }

    Ok(true)
}

fn is_older_than_15_minutes(timestamp: i64) -> bool {
    let timestamp_datetime = Utc.timestamp_opt(timestamp, 0).unwrap();
    let now = Utc::now();
    //TODO check if timestamp is greater than now
    let duration_since_timestamp = now.signed_duration_since(timestamp_datetime);
    duration_since_timestamp > Duration::minutes(15)
}

fn validate_root_key_exists(_wallet_metadata: &WalletMetadata) -> eyre::Result<bool> {
    //Check if root key exists
    // eyre::bail!("Root key does not exist")
    Ok(true)
}

fn store_client_key(_client_public_key: &str) -> eyre::Result<bool> {
    //Store client public key in a list
    Ok(true)
}

#[derive(Debug, Deserialize)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub path: String,
    pub hash: String,
}

pub async fn get_release(application: &String, version: &String) -> eyre::Result<Release> {
    let client = JsonRpcClient::connect("https://rpc.testnet.near.org");
    let request = methods::query::RpcQueryRequest {
        block_reference: BlockReference::Finality(Finality::Final),
        request: QueryRequest::CallFunction {
            account_id: "calimero-package-manager.testnet".parse()?,
            method_name: "get_release".to_string(),
            args: FunctionArgs::from(
                json!({
                    "name": application,
                    "version": version
                })
                .to_string()
                .into_bytes(),
            ),
        },
    };

    let response = client.call(request).await?;
    if let QueryResponseKind::CallResult(result) = response.kind {
        return Ok(from_slice::<Release>(&result.result)?);
    } else {
        eyre::bail!("Failed to fetch data from the rpc endpoint")
    }
}

pub async fn download_release(release: Release) -> eyre::Result<()> {
    let app_path = "";
    //verify_release(release, blob);
    Ok(())
}

pub async fn verify_release(release: Release, blob: String) {}

pub async fn install_application(application: &String, version: &String) -> eyre::Result<()> {
    download_release(get_release(application, version).await?).await
}

async fn install_application_handler(session: Session, Json(req): Json<InstallApplicationRequest>) {
    install_application(&req.application, &req.version).await;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PubKeyRequest {
    // unused ATM, uncomment when used
    // account_id: String,
    public_key: String,
    signature: String,
    callback_url: String,
}

#[derive(Deserialize)]
struct InstallApplicationRequest {
    application: String,
    version: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddClientKeyRequest {
    wallet_signature: String,
    payload: Payload,
    wallet_metadata: WalletMetadata,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    message: SignatureMessage,
    metadata: SignatureMetadataEnum,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SignatureMessage {
    nonce: String,
    application_id: String,
    timestamp: i64,
    node_signature: String,
    message: String,
    client_public_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WalletMetadata {
    #[serde(rename = "type")]
    wallet_type: WalletType,
    signing_key: String,
}

#[derive(Debug, Deserialize, PartialEq)]
enum WalletType {
    NEAR,
    ETH,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NearMetadata {
    #[serde(rename = "type")]
    wallet_type: WalletType,
    signing_key: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EthMetadata {
    #[serde(rename = "type")]
    wallet_type: WalletType,
    signing_key: String, // eth account 0x...
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "data")]
enum SignatureMetadataEnum {
    NEAR(NearSignatureMessageMetadata),
    ETH(EthSignatureMessageMetadata),
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct NearSignatureMessageMetadata {
    recipient: String,
    callback_url: String,
    nonce: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EthSignatureMessageMetadata {}

// Intermediate structs for initial parsing
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntermediateAddClientKeyRequest {
    wallet_signature: String,
    payload: IntermediatePayload,
    wallet_metadata: WalletMetadata, // Reuse WalletMetadata as it fits the intermediate step
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntermediatePayload {
    message: SignatureMessage, // Reuse SignatureMessage as it fits the intermediate step
    metadata: Value,           // Raw JSON value for the metadata
}

fn transform_request(
    intermediate: IntermediateAddClientKeyRequest,
) -> Result<AddClientKeyRequest, serde_json::Error> {
    let metadata_enum = match intermediate.wallet_metadata.wallet_type {
        WalletType::NEAR => {
            let metadata = serde_json::from_value::<NearSignatureMessageMetadata>(
                intermediate.payload.metadata.clone(),
            )?;
            SignatureMetadataEnum::NEAR(metadata)
        }
        WalletType::ETH => {
            let metadata = serde_json::from_value::<EthSignatureMessageMetadata>(
                intermediate.payload.metadata.clone(),
            )?;
            SignatureMetadataEnum::ETH(metadata)
        }
    };

    Ok(AddClientKeyRequest {
        wallet_signature: intermediate.wallet_signature,
        payload: Payload {
            message: intermediate.payload.message,
            metadata: metadata_enum,
        },
        wallet_metadata: intermediate.wallet_metadata,
    })
}
