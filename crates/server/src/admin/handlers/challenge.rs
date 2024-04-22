use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use libp2p::identity::Keypair;
use rand::{thread_rng, RngCore};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use tracing::error;

use crate::admin::service::{AdminState, ApiError, ApiResponse};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestChallenge {
    pub(crate) application_id: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestChallengeResponse {
    data: NodeChallenge,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodeChallenge {
    #[serde(flatten)]
    pub(crate) message: NodeChallengeMessage,
    pub(crate) node_signature: String,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NodeChallengeMessage {
    pub(crate) nonce: String,
    pub(crate) application_id: String, //optional if challenge is used on admin level
    pub(crate) timestamp: i64,
}

pub const CHALLENGE_KEY: &str = "challenge";

pub async fn request_challenge_handler(
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<RequestChallenge>,
) -> impl IntoResponse {
    if let Some(challenge) = session.get::<String>(CHALLENGE_KEY).await.ok().flatten() {
        match serde_json::from_str::<NodeChallenge>(&challenge) {
            Ok(challenge) => ApiResponse {
                payload: RequestChallengeResponse { data: challenge },
            }
            .into_response(),
            Err(_) => ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Failed to deserialize challenge".to_string(),
            }
            .into_response(),
        }
    } else {
        match generate_challenge(req.application_id.clone(), &state.keypair) {
            Ok(challenge) => {
                if let Err(err) = session.insert(CHALLENGE_KEY, &challenge).await {
                    error!("Failed to insert challenge into session: {}", err);
                    return ApiError {
                        status_code: StatusCode::INTERNAL_SERVER_ERROR,
                        message: "Failed to insert challenge into session".to_string(),
                    }
                    .into_response();
                }
                ApiResponse {
                    payload: RequestChallengeResponse { data: challenge },
                }
                .into_response()
            }
            Err(err) => {
                error!("Failed to generate client challenge: {}", err);
                ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to generate challenge".to_string(),
                }
                .into_response()
            }
        }
    }
}

fn generate_challenge(
    application_id: Option<String>,
    keypair: &Keypair,
) -> Result<NodeChallenge, ApiError> {
    let random_bytes = generate_random_bytes();
    let encoded = STANDARD.encode(&random_bytes);

    let node_challenge_message = NodeChallengeMessage {
        nonce: encoded,
        application_id: application_id.unwrap_or_default(),
        timestamp: chrono::Utc::now().timestamp(),
    };

    let message_vec = serde_json::to_vec(&node_challenge_message).map_err(|_| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: "Failed to serialize challenge data".into(),
    })?;

    match keypair.sign(&message_vec) {
        Ok(signature) => {
            let node_signature = STANDARD.encode(&signature);
            Ok(NodeChallenge {
                message: node_challenge_message,
                node_signature,
            })
        }
        Err(e) => Err(ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to sign challenge: {}", e),
        }),
    }
}

fn generate_random_bytes() -> [u8; 32] {
    let mut rng = thread_rng();
    let mut buf = [0u8; 32];
    rng.fill_bytes(&mut buf);
    buf
}
