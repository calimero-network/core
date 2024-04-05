use crate::verifysignature::verify_near_signature;
use axum::{http::StatusCode, response::IntoResponse, Json};
use calimero_identity::auth::verify_eth_signature;
use chrono::{Duration, TimeZone, Utc};
use serde::Deserialize;
use serde_json::Value;
use tower_sessions::Session;
use tracing::error;

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
pub(crate) struct IntermediateAddClientKeyRequest {
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

//* Register client key to authenticate client requests  */
pub async fn add_client_key_handler(
    _session: Session,
    Json(intermediate_req): Json<IntermediateAddClientKeyRequest>,
) -> impl IntoResponse {
    let req: Result<AddClientKeyRequest, (StatusCode, &str)> = transform_request(intermediate_req)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid payload"));

    if let Err(err) = req {
        return err;
    }

    let req: AddClientKeyRequest = req.unwrap();

    if let Err(err) = validate_root_key_exists(&req.wallet_metadata) {
        error!("Error with wallet key: {:?}", err.to_string());
        return (
            StatusCode::UNAUTHORIZED,
            "First add wallet public key to node root keys!",
        );
    }

    if let Err(err) = validate_challenge(req.wallet_metadata, &req.wallet_signature, &req.payload) {
        error!("Error with challenge: {:?}", err.to_string());
        return (StatusCode::BAD_REQUEST, "Invalid challenge!");
    }

    // Extract clientPublicKey and add it to list of client keys
    if let Err(err) = store_client_key(&req.payload.message.client_public_key) {
        error!("Error with storing client key: {:?}", err.to_string());
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Issue while storing client key",
        );
    }

    (StatusCode::OK, "\"data\":\"ok\"")
}

fn verify_node_signature(
    wallet_metadata: WalletMetadata,
    wallet_signature: &str,
    payload: &Payload,
) -> eyre::Result<bool> {
    match wallet_metadata.wallet_type {
        WalletType::NEAR => {
            let near_metadata: &NearSignatureMessageMetadata = match &payload.metadata {
                SignatureMetadataEnum::NEAR(metadata) => metadata,
                _ => eyre::bail!("Invalid metadata"),
            };

            let result = verify_near_signature(
                &payload.message.nonce,
                &payload.message.message,
                &near_metadata.recipient,
                &near_metadata.callback_url,
                &wallet_signature,
                &wallet_metadata.signing_key,
            );

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

            verify_eth_signature(
                &wallet_metadata.signing_key,
                &payload.message.message,
                wallet_signature,
            )?;

            Ok(true)
        }
    }
}

//Check if challenge is valid
fn validate_challenge(
    wallet_metadata: WalletMetadata,
    wallet_signature: &str,
    payload: &Payload,
) -> eyre::Result<bool> {
    validate_challenge_content(&payload)?;

    // Check if node has created signature
    verify_node_signature(wallet_metadata, &wallet_signature, &payload)?;

    // Check challenge to verify if it has expired or not
    if is_older_than_15_minutes(payload.message.timestamp) {
        eyre::bail!("Challenge is too old. Please request a new challenge.")
    }

    Ok(true)
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
