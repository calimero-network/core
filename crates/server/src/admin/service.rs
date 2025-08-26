use core::error::Error;
use core::fmt::{self, Display, Formatter};
use core::str::from_utf8;
use std::str;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Extension, Router};
use eyre::Report;
use rust_embed::{EmbeddedFile, RustEmbed};
use serde::{Deserialize, Serialize};
use serde_json::{json, to_string as to_json_string};
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

use super::handlers::context::{grant_capabilities, revoke_capabilities};
use super::handlers::proposals::{
    get_context_storage_entries_handler, get_context_value_handler,
    get_number_of_active_proposals_handler, get_number_of_proposal_approvals_handler,
    get_proposal_approvers_handler, get_proposal_handler, get_proposals_handler,
    get_proxy_contract_handler,
};
use super::handlers::{alias, blob};
use super::storage::ssl::get_ssl;
use crate::admin::handlers::applications::{
    get_application, install_application, install_dev_application, list_applications,
    uninstall_application,
};
use crate::admin::handlers::context::{
    create_context, delete_context, get_context, get_context_identities, get_context_storage,
    get_contexts, invite_to_context, join_context, sync, update_context_application,
};
use crate::admin::handlers::identity::generate_context_identity;
use crate::admin::handlers::peers::get_peers_count_handler;
use crate::config::ServerConfig;
#[cfg(feature = "host_layer")]
use crate::middleware::host::HostLayer;
use crate::AdminState;

#[derive(Clone, Debug, Deserialize, Serialize)]
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

// Embed the contents of the admin-ui build directory into the binary
#[derive(RustEmbed)]
#[folder = "$CALIMERO_WEBUI_PATH"]
struct NodeUiStaticFiles;

#[expect(
    clippy::too_many_lines,
    reason = "Acceptable here - mostly repetitive setup"
)]
pub(crate) fn setup(
    config: &ServerConfig,
    shared_state: Arc<AdminState>,
) -> Option<(String, Router)> {
    let _ = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin api is disabled");
            return None;
        }
    };

    let base_path = "/admin-api";

    // Get the node prefix from env var
    let admin_path = if let Ok(prefix) = std::env::var("NODE_PATH_PREFIX") {
        format!("{}{}", prefix, base_path)
    } else {
        base_path.to_owned()
    };

    for listen in &config.listen {
        info!(
            "Admin API server listening on {}/http{{{}}}",
            listen, admin_path
        );
    }

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    let router = Router::new()
        // Application management
        .route("/install-application", post(install_application::handler))
        .route(
            "/install-dev-application",
            post(install_dev_application::handler),
        )
        .route("/applications", get(list_applications::handler))
        .route(
            "/applications/:application_id",
            get(get_application::handler).delete(uninstall_application::handler),
        )
        // Context management
        .route(
            "/contexts",
            get(get_contexts::handler).post(create_context::handler),
        )
        .route("/contexts/invite", post(invite_to_context::handler))
        .route("/contexts/join", post(join_context::handler))
        .route(
            "/contexts/:context_id",
            get(get_context::handler).delete(delete_context::handler),
        )
        .route(
            "/contexts/:context_id/application",
            post(update_context_application::handler),
        )
        .route(
            "/contexts/:context_id/storage",
            get(get_context_storage::handler),
        )
        .route(
            "/contexts/:context_id/identities",
            get(get_context_identities::handler),
        )
        .route(
            "/contexts/:context_id/identities-owned",
            get(get_context_identities::handler),
        )
        .route(
            "/contexts/:context_id/capabilities/grant",
            post(grant_capabilities::handler),
        )
        .route(
            "/contexts/:context_id/capabilities/revoke",
            post(revoke_capabilities::handler),
        )
        // Identity management
        .route(
            "/identity/context",
            post(generate_context_identity::handler),
        )
        // Proposals
        .route(
            "/contexts/:context_id/proposals/:proposal_id/approvals/count",
            get(get_number_of_proposal_approvals_handler),
        )
        .route(
            "/contexts/:context_id/proposals/:proposal_id/approvals/users",
            get(get_proposal_approvers_handler),
        )
        .route(
            "/contexts/:context_id/proposals/count",
            get(get_number_of_active_proposals_handler),
        )
        .route(
            "/contexts/:context_id/proposals",
            post(get_proposals_handler),
        )
        .route(
            "/contexts/:context_id/proposals/:proposal_id",
            get(get_proposal_handler),
        )
        .route(
            "/contexts/:context_id/proposals/get-context-value",
            post(get_context_value_handler),
        )
        .route(
            "/contexts/:context_id/proposals/context-storage-entries",
            post(get_context_storage_entries_handler),
        )
        .route(
            "/contexts/:context_id/proxy-contract",
            get(get_proxy_contract_handler),
        )
        .nest(
            "/contexts/sync",
            Router::new()
                .route("/", post(sync::handler))
                .route("/:context_id", post(sync::handler)),
        )
        // Network info
        .route("/peers", get(get_peers_count_handler))
        // Blob management
        .route("/blobs", put(blob::upload_handler).get(blob::list_handler))
        .route(
            "/blobs/:blob_id",
            get(blob::download_handler)
                .head(blob::info_handler)
                .delete(blob::delete_handler),
        )
        // Alias management
        .nest("/alias", alias::service())
        .route("/health", get(health_check_handler))
        // Dummy endpoint used to figure out if we are running behind auth or not
        .route("/is-authed", get(is_authed_handler))
        .route("/certificate", get(certificate_handler))
        .layer(Extension(Arc::clone(&shared_state)));

    let admin_router = Router::new()
        .merge(router)
        .layer(Extension(shared_state))
        .layer(session_layer);

    #[cfg(feature = "host_layer")]
    let admin_router = admin_router.layer(HostLayer::new(config.listen.clone()));

    Some((admin_path, admin_router))
}

/// Creates a router for serving static node-ui files and providing fallback to `index.html` for SPA routing.
///
/// This function checks if the admin dashboard is enabled in the provided configuration.
/// If the admin site is enabled, it returns a router that serves embedded static files
/// and routes all SPA-related requests (like `/admin-dashboard/`) to `index.html`.
///
/// # Parameters
/// - `config`: A reference to the server configuration that contains the admin site settings.
///
/// # Returns
/// - `Option<(String, Router)>`: If the admin site is enabled, it returns a tuple containing
///   the base path (e.g., "/admin-dashboard" or with prefix from NODE_PATH_PREFIX env var)
///   and the router for that path. If the admin site is disabled, it returns `None`.
pub(crate) fn site(config: &ServerConfig) -> Option<(String, Router)> {
    let _admin_config = match &config.admin {
        Some(config) if config.enabled => config,
        _ => {
            info!("Admin site is disabled");
            return None;
        }
    };

    let base_path = "/admin-dashboard";

    // First check the environment variable, fall back to config if not present
    let path = if let Ok(prefix) = std::env::var("NODE_PATH_PREFIX") {
        info!("Using path prefix from environment: {}", prefix);
        format!("{}{}", prefix, base_path)
    } else {
        info!("No path prefix configured");
        base_path.to_owned()
    };

    for listen in &config.listen {
        info!(
            "Admin Dashboard UI available on {}/http{{{}}}",
            listen, path
        );
    }

    // Create a router to serve static files and fallback to index.html
    let router = Router::new()
        .route("/", get(serve_embedded_file)) // Match base path
        .route("/*path", get(serve_embedded_file)); // Match all sub-paths

    Some((path, router))
}

/// Serves embedded static files or falls back to `index.html` for SPA routing.
///
/// This function handles requests by removing the "/admin-dashboard/" prefix from the requested URI path,
/// and then attempting to serve the requested file from the embedded directory. If the requested file
/// is not found, it serves `index.html` to support client-side routing.
///
/// # Parameters
/// - `uri`: The requested URI, which will be used to determine the file path in the embedded directory.
///
/// # Returns
/// - `Result<impl IntoResponse, StatusCode>`: If the requested file is found or the fallback to index.html
///   succeeds, it returns an `Ok` with the response. If no file can be served, it returns an `Err` with
///   a 404 NOT_FOUND status code.
async fn serve_embedded_file(uri: Uri) -> Result<impl IntoResponse, StatusCode> {
    // Get the full path prefix (node prefix + admin dashboard)
    let full_prefix = if let Ok(node_prefix) = std::env::var("NODE_PATH_PREFIX") {
        format!("{}/admin-dashboard/", node_prefix)
    } else {
        "/admin-dashboard/".to_owned()
    };

    // Extract the path from the URI, removing the full prefix and any leading slashes
    let path = uri
        .path()
        .trim_start_matches(&full_prefix)
        .trim_start_matches('/');

    // Use "index.html" for empty paths (root requests)
    let path = if path.is_empty() { "index.html" } else { path };

    // Attempt to serve the requested file
    if let Some(file) = NodeUiStaticFiles::get(path) {
        return serve_file(file);
    }

    // Fallback to index.html for SPA routing if the file wasn't found and it's not already "index.html"
    if path != "index.html" {
        if let Some(index_file) = NodeUiStaticFiles::get("index.html") {
            return serve_file(index_file);
        }
    }

    // Return 404 if the file is not found and we can't fallback to index.html
    Err(StatusCode::NOT_FOUND)
}

/// Serves a static file with the correct MIME type.
///
/// This function builds a `Response` with the appropriate content type for the given file
/// and serves the file's content.
///
/// # Parameters
/// - `file`: A reference to the `EmbeddedFile` to be served.
///
/// # Returns
/// - `Result<impl IntoResponse, StatusCode>`: If the response is successfully built, it returns an `Ok`
///   with the response. If there is an error building the response, it returns an `Err` with a
///   500 INTERNAL_SERVER_ERROR status code.
fn serve_file(file: EmbeddedFile) -> Result<impl IntoResponse, StatusCode> {
    let content = if let Ok(content) = String::from_utf8(file.data.to_vec()) {
        // Process HTML, JavaScript, and CSS files
        if file.metadata.mimetype().starts_with("text/html")
            || file
                .metadata
                .mimetype()
                .starts_with("application/javascript")
            || file.metadata.mimetype() == "text/css"
        {
            // Get the node prefix from env var
            let base_path = if let Ok(prefix) = std::env::var("NODE_PATH_PREFIX") {
                format!("{}/admin-dashboard", prefix)
            } else {
                "/admin-dashboard".to_owned()
            };

            // Replace all instances of /admin-dashboard with the correct base path
            let modified_content = content
                .replace("\"/admin-dashboard", &format!("\"{}", base_path))
                .replace("'/admin-dashboard", &format!("'{}", base_path))
                .replace("(/admin-dashboard", &format!("({}", base_path))
                .replace(" /admin-dashboard", &format!(" {}", base_path))
                .replace(
                    "href=\"/admin-dashboard",
                    &format!("href=\"{}/admin-dashboard", base_path),
                )
                .replace(
                    "src=\"/admin-dashboard",
                    &format!("src=\"{}/admin-dashboard", base_path),
                );
            modified_content.into_bytes()
        } else {
            file.data.into_owned()
        }
    } else {
        file.data.into_owned()
    };
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", file.metadata.mimetype())
        .body(Body::from(content))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[expect(clippy::exhaustive_structs, reason = "Exhaustive")]
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
#[derive(Debug, Serialize)]
struct IsAuthedResponse {
    data: IsAuthed,
}

#[derive(Debug, Serialize)]
struct IsAuthed {
    status: String,
}

async fn is_authed_handler() -> impl IntoResponse {
    ApiResponse {
        payload: IsAuthedResponse {
            data: IsAuthed {
                status: "alive".to_owned(),
            },
        },
    }
    .into_response()
}

async fn certificate_handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    #[expect(clippy::print_stderr, reason = "Acceptable for CLI")]
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
