use core::fmt::{self, Display, Formatter};
use core::str::from_utf8;
use std::error::Error;
use std::str;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use calimero_context::ContextManager;
use calimero_store::Store;
use eyre::Report;
use libp2p::identity::Keypair;
use serde::{Deserialize, Serialize};
use serde_json::{json, to_string as to_json_string};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_status::SetStatus;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

use super::storage::ssl::get_ssl;
use crate::admin::handlers::add_client_key::add_client_key_handler;
use crate::admin::handlers::applications::{
    get_application, get_application_details_handler, install_application_handler,
    install_dev_application_handler, list_applications_handler,
};
use crate::admin::handlers::challenge::request_challenge_handler;
use crate::admin::handlers::context::{
    create_context_handler, delete_context_handler, get_context_client_keys_handler,
    get_context_handler, get_context_identities_handler, get_context_storage_handler,
    get_context_users_handler, get_contexts_handler, join_context_handler, update_application_id,
};
use crate::admin::handlers::fetch_did::fetch_did_handler;
use crate::admin::handlers::root_keys::{create_root_key_handler, delete_auth_keys_handler};
use crate::config::ServerConfig;
use crate::middleware::auth::AuthSignatureLayer;
use crate::middleware::host::HostLayer;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct AdminConfig {
    #[serde(default = "calimero_primitives::common::bool_true")]
    pub enabled: bool,
}

impl AdminConfig {
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self { enabled }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub struct AdminState {
    pub store: Store,
    pub keypair: Keypair,
    pub ctx_manager: ContextManager,
}

pub(crate) fn setup(
    config: &ServerConfig,
    store: Store,
    ctx_manager: ContextManager,
) -> Option<(&'static str, Router)> {
    let _ = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin api is disabled");
            return None;
        }
    };

    let admin_path = "/admin-api";

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    let shared_state = Arc::new(AdminState {
        store: store.clone(),
        keypair: config.identity.clone(),
        ctx_manager,
    });
    let protected_router = Router::new()
        .route("/root-key", post(create_root_key_handler))
        .route("/install-application", post(install_application_handler))
        .route("/applications", get(list_applications_handler))
        .route(
            "/applications/:app_id",
            get(get_application_details_handler),
        )
        .route("/did", get(fetch_did_handler))
        .route("/contexts", post(create_context_handler))
        .route("/contexts/:context_id", delete(delete_context_handler))
        .route("/contexts/:context_id", get(get_context_handler))
        .route(
            "/contexts/:context_id/users",
            get(get_context_users_handler),
        )
        .route(
            "/contexts/:context_id/client-keys",
            get(get_context_client_keys_handler),
        )
        .route(
            "/contexts/:context_id/storage",
            get(get_context_storage_handler),
        )
        .route(
            "/contexts/:context_id/identities",
            get(get_context_identities_handler),
        )
        .route("/contexts/:context_id/join", post(join_context_handler))
        .route("/contexts", get(get_contexts_handler))
        .route("/identity/keys", delete(delete_auth_keys_handler))
        .layer(AuthSignatureLayer::new(store))
        .layer(Extension(Arc::clone(&shared_state)));

    let unprotected_router = Router::new()
        .route("/health", get(health_check_handler))
        .route("/certificate", get(certificate_handler))
        .route("/request-challenge", post(request_challenge_handler))
        .route("/add-client-key", post(add_client_key_handler))
        .route(
            "/dev/install-application",
            post(install_dev_application_handler),
        )
        .route("/dev/application/:application_id", get(get_application))
        .route(
            "/dev/contexts",
            get(get_contexts_handler).post(create_context_handler),
        )
        .route("/dev/contexts/:context_id/join", post(join_context_handler))
        .route(
            "/dev/contexts/:context_id/application",
            post(update_application_id),
        )
        .route("/dev/applications", get(list_applications_handler))
        .layer(Extension(shared_state));

    let admin_router = Router::new()
        .nest("/", unprotected_router)
        .nest("/", protected_router)
        .layer(session_layer)
        .layer(HostLayer::new(config.listen.clone()));

    Some((admin_path, admin_router))
}

pub(crate) fn site(
    config: &ServerConfig,
) -> Option<(&'static str, ServeDir<SetStatus<ServeFile>>)> {
    let _config = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin site is disabled");
            return None;
        }
    };
    let path = "/admin-dashboard";

    let react_static_files_path = "./node-ui/build";
    let react_app_serve_dir = ServeDir::new(react_static_files_path).not_found_service(
        ServeFile::new(format!("{react_static_files_path}/index.html")),
    );

    Some((path, react_app_serve_dir))
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[allow(clippy::exhaustive_structs)]
pub struct Empty;

#[derive(Debug)]
pub struct ApiResponse<T: Serialize> {
    pub(crate) payload: T,
}

impl<T> IntoResponse for ApiResponse<T>
where
    T: Serialize,
{
    fn into_response(self) -> Response<Body> {
        //TODO add data to response
        let body = to_json_string(&self.payload).unwrap();
        Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/json")
            .body(Body::from(body))
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
    fn into_response(self) -> Response<Body> {
        let body = json!({ "error": self.message }).to_string();
        Response::builder()
            .status(&self.status_code)
            .header("Content-Type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }
}

#[must_use]
pub fn parse_api_error(err: Report) -> ApiError {
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
                status: "alive".to_owned(),
            },
        },
    }
    .into_response()
}

async fn certificate_handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    #[allow(clippy::print_stderr)]
    let certificate = match get_ssl(&state.store) {
        Ok(Some(cert)) => Some(cert),
        Ok(None) => None,
        Err(err) => {
            eprintln!("Failed to get the certificate: {err}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to get the certificate",
            )
                .into_response();
        }
    };

    if let Some(certificate) = certificate {
        // Generate the file content
        let file_content = match from_utf8(certificate.cert()) {
            Ok(content) => content.to_owned(),
            Err(_) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Failed to read certificate content",
                )
                    .into_response()
            }
        };
        let file_name = "cert.pem";

        // Create headers for file download
        let mut headers = HeaderMap::new();
        drop(headers.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain")));
        drop(headers.insert(
            header::CONTENT_DISPOSITION,
            HeaderValue::from_str(&format!("attachment; filename=\"{file_name}\"")).unwrap(),
        ));

        // Create the response with the file content and headers
        (headers, file_content).into_response()
    } else {
        (StatusCode::NOT_FOUND, "Certificate not found").into_response()
    }
}
