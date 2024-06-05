use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use calimero_identity::auth::verify_eth_signature;
use calimero_primitives::identity::{RootKey, WalletType};
use calimero_server_primitives::admin::ApplicationListResult;
use calimero_store::Store;
use chrono::Utc;
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use tracing::{error, info};

use super::handlers::add_client_key::{add_client_key_handler, WalletMetadata};
use super::handlers::challenge::{request_challenge_handler, NodeChallenge, CHALLENGE_KEY};
use super::handlers::context::{
    create_context_handler, delete_context_handler, get_context_handler, get_contexts_handler,
};
use super::handlers::fetch_did::fetch_did_handler;
use super::storage::root_key::add_root_key;
use crate::verifysignature;

#[derive(Debug, Serialize, Deserialize)]
pub struct AdminConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

pub struct AdminState {
    pub store: Store,
    pub keypair: Keypair,
    pub application_manager: calimero_application::ApplicationManager,
}

pub(crate) fn setup(
    config: &crate::config::ServerConfig,
    store: Store,
    application_manager: calimero_application::ApplicationManager,
) -> eyre::Result<Option<(&'static str, Router)>> {
    match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin api is disabled");
            return Ok(None);
        }
    };

    let admin_path = "/admin-api";

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store);

    let shared_state = Arc::new(AdminState {
        store,
        keypair: config.identity.clone(),
        application_manager,
    });

    let admin_router = Router::new()
        .route("/health", get(health_check_handler))
        .route("/root-key", post(create_root_key_handler))
        .route("/request-challenge", post(request_challenge_handler))
        .route("/install-application", post(install_application_handler))
        .route("/applications", get(list_applications_handler))
        .route("/add-client-key", post(add_client_key_handler))
        .route("/did", get(fetch_did_handler))
        .route("/contexts", post(create_context_handler))
        .route("/contexts/:context_id", delete(delete_context_handler))
        .route("/contexts/:context_id", get(get_context_handler))
        .route("/contexts", get(get_contexts_handler))
        .layer(Extension(shared_state))
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

pub struct ApiResponse<T: Serialize> {
    pub(crate) payload: T,
}

impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        //TODO add data to response
        let body = serde_json::to_string(&self.payload).unwrap();
        axum::http::Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    }
}

#[derive(Debug)]
pub struct ApiError {
    pub(crate) status_code: StatusCode,
    pub(crate) message: String,
}

impl Display for ApiError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.status_code, self.message)
    }
}

impl Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::http::Response<axum::body::Body> {
        let body = json!({ "error": self.message }).to_string();
        axum::http::Response::builder()
            .status(&self.status_code)
            .header("Content-Type", "application/json")
            .body(axum::body::Body::from(body))
            .unwrap()
    }
}

#[derive(Debug, Serialize)]
struct GetHealthResponse {
    data: HealthStatus,
}

#[derive(Debug, Serialize)]
struct HealthStatus {
    status: String,
}

async fn health_check_handler() -> impl IntoResponse {
    ApiResponse {
        payload: GetHealthResponse {
            data: HealthStatus {
                status: "alive".to_string(),
            },
        },
    }
    .into_response()
}

#[derive(Debug, Serialize)]
struct RootKeyResponse {
    data: String,
}

fn handle_root_key_result(result: eyre::Result<bool>) -> axum::http::Response<axum::body::Body>{
    match result {
        Ok(_) => {
            info!("Root key added");
            ApiResponse { payload: RootKeyResponse { data: "Root key added".to_string()} }.into_response()
        }
        Err(e) => {
            error!("Failed to store root key: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Failed to store root key").into_response()
        }
    }
}

async fn create_root_key_handler(
    session: Session,
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<PubKeyRequest>,
) -> impl IntoResponse {
    let recipient = "me";
    match session
        .get::<NodeChallenge>(CHALLENGE_KEY)
        .await
        .ok()
        .flatten()
    {
        Some(challenge) => {
            match req.wallet_metadata.wallet_type {
                WalletType::NEAR => {
                    if !verifysignature::verify_near_signature(
                        &challenge.message.nonce,
                        &challenge.node_signature,
                        recipient,
                        &req.callback_url,
                        &req.signature,
                        &req.public_key,
                    ) {
                        return (StatusCode::BAD_REQUEST, "Invalid signature").into_response();
                    }

                    let result = add_root_key(
                        &state.store,
                        RootKey {
                            signing_key: req.public_key,
                            wallet_type: WalletType::NEAR,
                            created_at: Utc::now().timestamp_millis() as u64
                        },
                    );

                    handle_root_key_result(result)
                }
                WalletType::ETH { .. } => {
                    if let Err(_) = verify_eth_signature(
                        &req.wallet_metadata.signing_key,
                        &req.message,
                        &req.signature
                    ) {
                        return (StatusCode::BAD_REQUEST, "Invalid signature").into_response();
                    }

                    let result = add_root_key(
                        &state.store,
                        RootKey {
                            signing_key: req.public_key,
                            wallet_type: req.wallet_metadata.wallet_type,
                            created_at: Utc::now().timestamp_millis() as u64
                        }
                    );

                    handle_root_key_result(result)
                }
            }
        }
        _ => (StatusCode::BAD_REQUEST, "Challenge not found").into_response(),
    }
}

async fn install_application_handler(
    Extension(state): Extension<Arc<AdminState>>,
    Json(req): Json<calimero_server_primitives::admin::InstallApplicationRequest>,
) -> impl IntoResponse {
    match state
        .application_manager
        .install_application(req.application, &req.version)
        .await
    {
        Ok(()) => ApiResponse { payload: () }.into_response(),
        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[derive(Debug, Serialize)]
struct ListApplicationsResponse {
    data: ApplicationListResult,
}

async fn list_applications_handler(
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    match state
        .application_manager
        .list_installed_applications()
        .await
    {
        Ok(applications) => ApiResponse {
            payload: ListApplicationsResponse {
                data: ApplicationListResult { apps: applications},
            },
        }
        .into_response(),

        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PubKeyRequest {
    // unused ATM, uncomment when used
    // account_id: String,
    public_key: String,
    signature: String,
    callback_url: String,
    wallet_metadata: WalletMetadata,
    message: String
}
