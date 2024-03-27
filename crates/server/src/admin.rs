use axum::{
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rand::{thread_rng, RngCore};
use serde::{Deserialize, Serialize};
use tower_http::{
    services::{ServeDir, ServeFile},
    set_status::SetStatus,
};
use tower_sessions::Session;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

use crate::verifysignature;

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
        .route("/node-key", post(create_root_key_handler))
        .route("/request-challenge", post(request_challenge_handler))
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

pub async fn request_challenge_handler(session: Session) -> impl IntoResponse {
    if let Some(challenge) = session.get(CHALLENGE_KEY).await.unwrap_or(None) {
        (StatusCode::OK, challenge)
    } else {
        // No challenge in session, generate a new one
        let challenge = generate_challenge();
        session.insert(CHALLENGE_KEY, &challenge).await.unwrap();
        (StatusCode::OK, challenge)
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

async fn health_check_handler() -> Json<&'static str> {
    Json("{\"status\":\"ok\"}")
}

async fn create_root_key_handler(
    session: Session,
    Json(req): Json<PubKeyRequest>,
) -> impl IntoResponse {
    let message = "helloworld";
    let app = "me";
    let curl = "http://127.0.0.1:2428/admin/confirm-wallet";

    match session.get::<String>(CHALLENGE_KEY).await.unwrap_or(None) {
        Some(challenge) => {
            if verifysignature::verify_signature(
                &challenge,
                message,
                app,
                curl,
                &req.signature,
                &req.public_key,
            ) {
                (StatusCode::OK, Json("\"status\": \"Root key created\""))
            } else {
                (StatusCode::OK, Json("\"status\": \"Invalid signature\""))
            }
        }
        None => (
            StatusCode::BAD_REQUEST,
            Json("\"status\": \"No challenge found\""),
        ),
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PubKeyRequest {
    account_id: String,
    public_key: String,
    signature: String,
}
