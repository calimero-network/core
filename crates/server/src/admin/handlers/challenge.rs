use std::sync::Arc;

use axum::response::IntoResponse;
use axum::{Extension, Json};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_primitives::context::ContextId;
use calimero_server_primitives::admin::{AdminState, NodeChallenge, NodeChallengeMessage};
use chrono::Utc;
use libp2p::identity::Keypair;
use rand::{thread_rng, RngCore};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::to_vec as to_json_vec;
use tower_sessions::Session;
use tracing::error;

use crate::admin::service::{ApiError, ApiResponse};

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestChallenge {
    pub(crate) context_id: Option<ContextId>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RequestChallengeResponse {
    data: NodeChallenge,
}

pub const CHALLENGE_KEY: &str = "challenge";

pub async fn request_challenge_handler(
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<RequestChallenge>,
) -> impl IntoResponse {
    match generate_challenge(req.context_id, &state.keypair) {
        Ok(challenge) => {
            if let Err(err) = session.insert(CHALLENGE_KEY, &challenge).await {
                error!("Failed to insert challenge into session: {}", err);
                return ApiError {
                    status_code: StatusCode::INTERNAL_SERVER_ERROR,
                    message: "Failed to insert challenge into session".to_owned(),
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
                message: "Failed to generate challenge".to_owned(),
            }
            .into_response()
        }
    }
}

fn generate_challenge(
    context_id: Option<ContextId>,
    keypair: &Keypair,
) -> Result<NodeChallenge, ApiError> {
    let random_bytes = generate_random_bytes();
    let encoded = STANDARD.encode(random_bytes);

    let node_challenge_message =
        NodeChallengeMessage::new(encoded, context_id, Utc::now().timestamp());

    let message_vec = to_json_vec(&node_challenge_message).map_err(|_| ApiError {
        status_code: StatusCode::INTERNAL_SERVER_ERROR,
        message: "Failed to serialize challenge data".into(),
    })?;

    match keypair.sign(&message_vec) {
        Ok(signature) => {
            let node_signature = STANDARD.encode(&signature);
            Ok(NodeChallenge::new(node_challenge_message, node_signature))
        }
        Err(e) => Err(ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: format!("Failed to sign challenge: {e}"),
        }),
    }
}

fn generate_random_bytes() -> [u8; 32] {
    let mut rng = thread_rng();
    let mut buf = [0_u8; 32];
    rng.fill_bytes(&mut buf);
    buf
}
