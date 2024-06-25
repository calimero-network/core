use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::Arc;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use calimero_server_primitives::admin::ApplicationListResult;
use calimero_store::Store;
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

use super::handlers;
use crate::middleware;

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
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    let shared_state = Arc::new(AdminState {
        store: store.clone(),
        keypair: config.identity.clone(),
        application_manager,
    });
    let protected_router = Router::new()
        .route(
            "/root-key",
            post(handlers::root_keys::create_root_key_handler),
        )
        .route("/install-application", post(install_application_handler))
        .route("/applications", get(list_applications_handler))
        .route("/did", get(handlers::fetch_did::fetch_did_handler))
        .route("/contexts", post(handlers::context::create_context_handler))
        .route(
            "/contexts/:context_id",
            delete(handlers::context::delete_context_handler),
        )
        .route(
            "/contexts/:context_id",
            get(handlers::context::get_context_handler),
        )
        .route(
            "/contexts/:context_id/users",
            get(handlers::context::get_context_users_handler),
        )
        .route(
            "/contexts/:context_id/client-keys",
            get(handlers::context::get_context_client_keys_handler),
        )
        .route(
            "/contexts/:context_id/storage",
            get(handlers::context::get_context_storage_handler),
        )
        .route("/contexts", get(handlers::context::get_contexts_handler))
        .route(
            "/identity/keys",
            delete(handlers::root_keys::delete_auth_keys_handler),
        )
        .layer(middleware::auth::AuthSignatureLayer::new(store))
        .layer(Extension(shared_state.clone()));

    let unprotected_router = Router::new()
        .route("/health", get(health_check_handler))
        .route(
            "/request-challenge",
            post(handlers::challenge::request_challenge_handler),
        )
        .route(
            "/add-client-key",
            post(handlers::add_client_key::add_client_key_handler),
        )
        .layer(Extension(shared_state));

    let admin_router = Router::new()
        .nest("/", unprotected_router)
        .nest("/", protected_router)
        .layer(session_layer);

    Ok(Some((admin_path, admin_router)))
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Empty {}

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

pub fn parse_api_error(err: eyre::Report) -> ApiError {
    match err.downcast::<ApiError>() {
        Ok(api_error) => api_error,
        Err(original_error) => ApiError {
            status_code: StatusCode::INTERNAL_SERVER_ERROR,
            message: original_error.to_string(),
        },
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
struct InstallApplicationResponse {
    data: bool,
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
        Ok(()) => ApiResponse {
            payload: InstallApplicationResponse { data: true },
        }
        .into_response(),
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
                data: ApplicationListResult { apps: applications },
            },
        }
        .into_response(),

        Err(err) => (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response(),
    }
}
