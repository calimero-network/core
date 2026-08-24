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
use bytes::Bytes;
use eyre::Report;
use rust_embed::{EmbeddedFile, RustEmbed};
use serde::{Deserialize, Serialize};
use serde_json::{json, to_string as to_json_string};
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

use super::handlers::{alias, blob, groups, namespaces, tee};
use super::storage::ssl::get_ssl;
use crate::admin::handlers::applications::{
    get_application, get_application_abi, install_application, install_dev_application,
    list_application_versions, list_applications, uninstall_application,
};
use crate::admin::handlers::context::{
    create_context, delete_context, get_context, get_context_group, get_context_identities,
    get_context_ids, get_context_storage, get_contexts_for_application,
    get_contexts_with_executors_for_application, join_context, leave_context, perform_intent,
    resync_context, sync, update_context_application,
};
use crate::admin::handlers::identity::{generate_context_identity, get_node_identity};
use crate::admin::handlers::network;
use crate::admin::handlers::packages::{get_latest_version, list_packages, list_versions};
use crate::admin::handlers::peers::get_peers_count_handler;
use crate::admin::handlers::usage;
use crate::config::ServerConfig;
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
) -> Option<(String, Router, Router)> {
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
        format!("{prefix}{base_path}")
    } else {
        base_path.to_owned()
    };

    for listen in &config.listen {
        info!("Admin API listening on {}/http{{{}}}", listen, admin_path);
    }

    let session_store = MemoryStore::default();
    let session_layer = SessionManagerLayer::new(session_store).with_secure(false);

    let protected_routes = Router::new()
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
        .route(
            "/applications/:application_id/abi",
            get(get_application_abi::handler),
        )
        .route(
            "/applications/:application_id/versions",
            get(list_application_versions::handler),
        )
        // Package management
        .route("/packages", get(list_packages::handler))
        .route("/packages/:package/versions", get(list_versions::handler))
        .route("/packages/:package/latest", get(get_latest_version::handler))
        // Context management
        .route(
            "/contexts",
            get(get_context_ids::handler).post(create_context::handler),
        )
        .route(
            "/contexts/:context_id",
            get(get_context::handler).delete(delete_context::handler),
        )
        .route(
            "/contexts/:context_id/application",
            post(update_context_application::handler),
        )
        .route(
            "/contexts/:context_id/intents",
            post(perform_intent::handler),
        )
        .route(
            "/contexts/:context_id/resync",
            post(resync_context::handler),
        )
        .route(
            "/contexts/for-application/:application_id",
            get(get_contexts_for_application::handler),
        )
        .route(
            "/contexts/with-executors/for-application/:application_id",
            get(get_contexts_with_executors_for_application::handler),
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
            "/contexts/:context_id/group",
            get(get_context_group::handler),
        )
        // Identity management
        .route(
            "/identity/context",
            post(generate_context_identity::handler),
        )
        .route("/identity", get(get_node_identity::handler))
        .nest(
            "/contexts/sync",
            Router::new()
                .route("/", post(sync::handler))
                .route("/:context_id", post(sync::handler)),
        )
        // Per-namespace usage (counts + on-disk bytes)
        .route("/usage", get(usage::handler))
        // libp2p connectivity snapshot (relays, rendezvous, DCUtR, AutoNAT)
        .route("/network/status", get(network::status::handler))
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
        // Group management
        .route("/groups", post(groups::create_group::handler))
        .route(
            "/groups/:group_id",
            get(groups::get_group_info::handler).delete(groups::delete_group::handler),
        )
        .route(
            "/groups/:group_id/contexts",
            get(groups::list_group_contexts::handler),
        )
        .route(
            "/groups/:group_id/reparent",
            post(groups::reparent_group::handler),
        )
        .route(
            "/groups/:group_id/subgroups",
            get(groups::list_subgroups::handler),
        )
        .route(
            "/groups/:group_id/members",
            get(groups::list_group_members::handler).post(groups::add_group_members::handler),
        )
        .route(
            "/groups/:group_id/members/remove",
            post(groups::remove_group_members::handler),
        )
        .route(
            "/groups/:group_id/member-devices",
            get(groups::list_member_devices::handler),
        )
        .route(
            "/groups/:group_id/leave",
            post(groups::leave_group::handler),
        )
        .route(
            "/groups/:group_id/members/:account/role",
            put(groups::update_member_role::handler),
        )
        .route(
            "/groups/:group_id/metadata",
            get(groups::set_group_metadata::get_handler).put(groups::set_group_metadata::handler),
        )
        .route(
            "/groups/:group_id/members/:account/metadata",
            get(groups::set_member_metadata::get_handler).put(groups::set_member_metadata::handler),
        )
        .route(
            "/groups/:group_id/contexts/:context_id/metadata",
            get(groups::set_context_metadata::get_handler)
                .put(groups::set_context_metadata::handler),
        )
        .route(
            "/groups/:group_id/contexts/:context_id/remove",
            post(groups::detach_context_from_group::handler),
        )
        .route(
            "/groups/:group_id/upgrade",
            post(groups::upgrade_group::handler),
        )
        .route(
            "/groups/:group_id/upgrade/status",
            get(groups::get_group_upgrade_status::handler),
        )
        .route(
            "/groups/:namespace_id/cascade-status",
            get(groups::get_cascade_status::handler),
        )
        .route(
            "/groups/:namespace_id/migration-status",
            get(groups::get_migration_status::handler),
        )
        .route(
            "/groups/:namespace_id/migration/abort",
            post(groups::abort_migration::handler),
        )
        .route(
            "/groups/:group_id/upgrade/retry",
            post(groups::retry_group_upgrade::handler),
        )
        .route(
            "/groups/:group_id/issue-ownership-proof",
            post(groups::issue_ownership_proof::handler),
        )
        .route(
            "/groups/:group_id/issue-namespace-ownership-proof",
            post(groups::issue_namespace_ownership_proof::handler),
        )
        .route(
            "/groups/:group_id/sync",
            post(groups::sync_group::handler),
        )
        .route(
            "/groups/:group_id/join-via-inheritance",
            post(groups::join_subgroup_inheritance::handler),
        )
        .route(
            "/contexts/:context_id/join",
            post(join_context::handler),
        )
        .route(
            "/contexts/:context_id/leave",
            post(leave_context::handler),
        )
        .route(
            "/groups/:group_id/members/:account/capabilities",
            get(groups::get_member_capabilities::handler)
                .put(groups::set_member_capabilities::handler),
        )
        .route(
            "/groups/:group_id/members/:account/auto-follow",
            put(groups::set_member_auto_follow::handler),
        )
        .route(
            "/groups/:group_id/settings/default-capabilities",
            put(groups::set_default_capabilities::handler),
        )
        .route(
            "/groups/:group_id/settings/tee-admission-policy",
            get(groups::get_tee_admission_policy::handler)
                .put(groups::set_tee_admission_policy::handler),
        )
        .route(
            "/groups/:group_id/settings/subgroup-visibility",
            put(groups::set_subgroup_visibility::handler),
        )
        // Legacy subgroup invitation/join routes kept for backwards compatibility.
        .route(
            "/groups/:group_id/invite",
            post(groups::create_group_invitation::handler),
        )
        .route("/groups/join", post(groups::join_group::handler))
        .route(
            "/namespaces/:namespace_id/account/pair-init",
            post(namespaces::pair_device_init::handler),
        )
        .route(
            "/namespaces/:namespace_id/account/pair-complete",
            post(namespaces::pair_device_complete::handler),
        )
        .route(
            "/namespaces/:namespace_id/account/revoke",
            post(namespaces::revoke_device::handler),
        )
        // Namespace management
        .route(
            "/namespaces",
            get(namespaces::list::handler).post(namespaces::create_namespace::handler),
        )
        .route(
            "/namespaces/:namespace_id",
            get(namespaces::get_namespace::handler).delete(namespaces::delete_namespace::handler),
        )
        .route(
            "/namespaces/:namespace_id/invite",
            post(namespaces::invite_namespace::handler),
        )
        .route(
            "/namespaces/:namespace_id/join",
            post(namespaces::join_namespace::handler),
        )
        .route(
            "/namespaces/:namespace_id/leave",
            post(namespaces::leave_namespace::handler),
        )
        .route(
            "/namespaces/for-application/:application_id",
            get(namespaces::list_for_application::handler),
        )
        // Namespace governance (Phase 2)
        .route(
            "/namespaces/:namespace_id/groups",
            get(namespaces::list_namespace_groups::handler)
                .post(namespaces::create_group_in_namespace::handler),
        )
        // TEE protected endpoints
        .nest("/tee", tee::protected_service())
        // Alias management
        .nest("/alias", alias::service())
        .layer(Extension(Arc::clone(&shared_state)))
        .layer(session_layer.clone());

    let public_routes = Router::new()
        .route("/health", get(health_check_handler))
        .route("/ready", get(readiness_check_handler))
        .route("/is-authed", get(is_authed_handler))
        .route("/certificate", get(certificate_handler))
        .nest("/tee", tee::service())
        .layer(Extension(shared_state));

    Some((admin_path, protected_routes, public_routes))
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
        format!("{prefix}{base_path}")
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
    // Extract the path from the URI, removing the full prefix and any leading slashes
    let path = uri
        .path()
        .trim_start_matches(dashboard_full_prefix())
        .trim_start_matches('/');

    // Use "index.html" for empty paths (root requests)
    let path = if path.is_empty() { "index.html" } else { path };

    // Attempt to serve the requested file
    if let Some(file) = NodeUiStaticFiles::get(path) {
        return serve_file(path, file);
    }

    // Fallback to index.html for SPA routing if the file wasn't found and it's not already "index.html"
    if path != "index.html" {
        if let Some(index_file) = NodeUiStaticFiles::get("index.html") {
            return serve_file("index.html", index_file);
        }
    }

    // Return 404 if the file is not found and we can't fallback to index.html
    Err(StatusCode::NOT_FOUND)
}

/// The `/admin-dashboard` base-path rewrite prefix, resolved from
/// `NODE_PATH_PREFIX` once per process. `None` when unset — the overwhelmingly
/// common case — which lets [`serve_file`] skip the rewrite (and its
/// full-content `.replace()` passes) entirely and serve the embedded bytes
/// as-is.
fn dashboard_base_prefix() -> Option<&'static str> {
    static PREFIX: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PREFIX
        .get_or_init(|| {
            std::env::var("NODE_PATH_PREFIX")
                .ok()
                .filter(|p| !p.is_empty())
        })
        .as_deref()
}

/// Full request-path prefix for dashboard assets
/// (`{NODE_PATH_PREFIX}/admin-dashboard/`), resolved once per process so
/// request handling never touches the environment.
fn dashboard_full_prefix() -> &'static str {
    static PREFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PREFIX.get_or_init(|| match dashboard_base_prefix() {
        Some(node_prefix) => format!("{node_prefix}/admin-dashboard/"),
        None => "/admin-dashboard/".to_owned(),
    })
}

/// Cache of rewritten text assets, keyed by embed path. Only populated when a
/// `NODE_PATH_PREFIX` is set (so the rewrite runs at most once per asset, not
/// once per request). Stored as [`Bytes`] so cache hits hand the body a
/// refcounted view instead of copying the asset. The embedded asset set is
/// fixed and small, so it needs no eviction.
fn rewritten_asset_cache() -> &'static std::sync::RwLock<std::collections::HashMap<String, Bytes>> {
    static CACHE: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, Bytes>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

fn is_rewritable_text(mimetype: &str) -> bool {
    mimetype.starts_with("text/html")
        || mimetype.starts_with("application/javascript")
        || mimetype == "text/css"
}

/// Apply the `/admin-dashboard` → `{prefix}/admin-dashboard` rewrites.
///
/// The `"`-quoted pass already covers `href="/admin-dashboard` and
/// `src="/admin-dashboard` occurrences (the old dedicated passes for those
/// could never match after it and were dead code).
fn rewrite_dashboard_paths(content: &str, prefix: &str) -> Vec<u8> {
    let base_path = format!("{prefix}/admin-dashboard");
    content
        .replace("\"/admin-dashboard", &format!("\"{base_path}"))
        .replace("'/admin-dashboard", &format!("'{base_path}"))
        .replace("(/admin-dashboard", &format!("({base_path}"))
        .replace(" /admin-dashboard", &format!(" {base_path}"))
        .into_bytes()
}

/// Serve an embedded static asset. Text assets (html/js/css) get the
/// `/admin-dashboard` base-path rewrite applied when a `NODE_PATH_PREFIX` is
/// configured (cached per path); everything else is served as-is.
fn serve_file(path: &str, file: EmbeddedFile) -> Result<Response<Body>, StatusCode> {
    let mimetype = file.metadata.mimetype().to_owned();

    let body = match (dashboard_base_prefix(), is_rewritable_text(&mimetype)) {
        // No prefix override, or a non-text asset: serve the embedded bytes
        // directly — no utf8 round-trip, no rewrite passes.
        (None, _) | (_, false) => Body::from(file.data.into_owned()),
        // Prefix override on a text asset: rewrite once, then cache by path so
        // subsequent requests reuse the transformed bytes.
        (Some(prefix), true) => {
            // Bind the lookup so the read guard drops here; in edition 2021 an
            // `if let` scrutinee temporary would outlive the whole `else`
            // branch and self-deadlock against the `.write()` below.
            let cached = rewritten_asset_cache()
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(path)
                .cloned();
            if let Some(cached) = cached {
                Body::from(cached)
            } else {
                let rewritten = match std::str::from_utf8(&file.data) {
                    Ok(text) => Bytes::from(rewrite_dashboard_paths(text, prefix)),
                    // Non-utf8 despite the text mimetype: serve raw, don't cache.
                    Err(_) => return build_asset_response(&mimetype, file.data.into_owned()),
                };
                let _ = rewritten_asset_cache()
                    .write()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .insert(path.to_owned(), rewritten.clone());
                Body::from(rewritten)
            }
        }
    };

    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mimetype)
        .body(body)
        .map_err(|e| {
            tracing::error!(error = %e, "failed to build file response");
            StatusCode::INTERNAL_SERVER_ERROR
        })
}

fn build_asset_response(mimetype: &str, content: Vec<u8>) -> Result<Response<Body>, StatusCode> {
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", mimetype)
        .body(Body::from(content))
        .map_err(|e| {
            tracing::error!(error = %e, "failed to build file response");
            StatusCode::INTERNAL_SERVER_ERROR
        })
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
            .status(self.status_code)
            .header("Content-Type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }
}

#[must_use]
pub fn parse_api_error(err: Report) -> ApiError {
    // A membership-gate rejection ("node is not a member of group X") is a
    // legitimate client-side precondition, not a server fault. Surface it as a
    // typed 403 with its (safe, intended) message instead of letting it fall
    // through to the generic 500 below. This is what a caller sees when it
    // lists a group the node hasn't joined / isn't in.
    if let Some(calimero_context::error::ContextError::NotAGroupMember { .. }) =
        err.downcast_ref::<calimero_context::error::ContextError>()
    {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: err.to_string(),
        };
    }
    // A warrant the gate refused, surfacing from inside the execute path. Every
    // variant is about the caller's authorization rather than this node's health,
    // and a replay is the one most likely to be hit in practice: a relay that
    // re-presents a spent warrant should be told `403`, not handed a `500` that
    // reads as "try again".
    if let Some(refusal) =
        err.downcast_ref::<calimero_governance_store::warrant_gate::WarrantRefusal>()
    {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: refusal.to_string(),
        };
    }
    // A delegated-intent refusal knows which kind of "no" it is — a malformed
    // request versus missing authority — and those ask opposite things of the
    // caller. Mapping both to the generic 500 below would tell a client the
    // server broke when in fact its warrant did.
    if let Some(refusal) =
        err.downcast_ref::<crate::admin::handlers::context::perform_intent::IntentRefusal>()
    {
        return ApiError {
            status_code: refusal.status(),
            message: err.to_string(),
        };
    }
    // The node cannot decide this op's authority YET — it is missing history or a
    // key it is entitled to, and the apply burned nothing, so the identical call
    // succeeds once it has caught up. `503` rather than the generic `500` because
    // the two ask opposite things of a client: retry me, versus stop. `Retry-After`
    // is deliberately omitted — how long depends on sync, which this layer cannot
    // estimate, and a wrong number is worse than none.
    if let Some(calimero_context::error::ContextError::AuthorityNotYetResolvable { .. }) =
        err.downcast_ref::<calimero_context::error::ContextError>()
    {
        return ApiError {
            status_code: StatusCode::SERVICE_UNAVAILABLE,
            message: err.to_string(),
        };
    }
    // The application's own `init` refused the call — nearly always because the
    // `initializationParams` do not match its signature. That is the caller's
    // input, not a server fault, and the guest's message is the only thing that
    // says what was wrong. Without this arm it falls through to the generic 500
    // below and the reason exists nowhere but the node's log, which is not
    // something an API client can read.
    if let Some(calimero_context::error::ContextError::InitFailed { .. }) =
        err.downcast_ref::<calimero_context::error::ContextError>()
    {
        return ApiError {
            status_code: StatusCode::BAD_REQUEST,
            message: err.to_string(),
        };
    }
    match err.downcast::<ApiError>() {
        Ok(api_error) => api_error,
        // An untyped error is an unexpected internal failure. Don't echo its
        // message back to the caller — it can carry store paths, key material,
        // or other internals. Log the detail server-side and return a generic
        // 500. (Typed `ApiError`s above keep their intended message/code.)
        Err(original_error) => {
            tracing::error!(error = ?original_error, "unhandled admin-api error");
            ApiError {
                status_code: StatusCode::INTERNAL_SERVER_ERROR,
                message: "Internal server error".to_owned(),
            }
        }
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

/// Liveness probe. Reports healthy only if the datastore answers a probe read —
/// a wedged store now surfaces as unhealthy instead of a constant "alive". This
/// is deliberately independent of the readiness lifecycle: liveness answers
/// "is the process still functioning" (a k8s liveness failure restarts the
/// pod), so a still-starting or draining node is live as long as its store
/// responds.
async fn health_check_handler(Extension(state): Extension<Arc<AdminState>>) -> impl IntoResponse {
    match state.store.ping() {
        Ok(()) => ApiResponse {
            payload: GetHealthResponse {
                data: HealthStatus {
                    status: "alive".to_owned(),
                },
            },
        }
        .into_response(),
        Err(err) => {
            tracing::warn!(%err, "health check: datastore ping failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                axum::Json(json!({ "data": { "status": "store_unavailable" } })),
            )
                .into_response()
        }
    }
}

/// Readiness probe. Returns 200 only when the node has finished starting AND is
/// not shutting down AND the datastore responds; otherwise 503 with the current
/// lifecycle label. A k8s readiness probe consults this to decide whether to
/// route traffic, so a Starting or ShuttingDown node is removed from the
/// service endpoints while it comes up or drains.
async fn readiness_check_handler(
    Extension(state): Extension<Arc<AdminState>>,
) -> impl IntoResponse {
    let lifecycle = state.readiness.label();
    let store_ok = state.store.ping().is_ok();
    if state.readiness.is_ready() && store_ok {
        (
            StatusCode::OK,
            axum::Json(json!({ "data": { "status": "ready" } })),
        )
            .into_response()
    } else {
        let status = if store_ok {
            lifecycle
        } else {
            "store_unavailable"
        };
        (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({ "data": { "status": status } })),
        )
            .into_response()
    }
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
    let certificate = match get_ssl(&state.store) {
        Ok(Some(cert)) => Some(cert),
        Ok(None) => None,
        Err(err) => {
            tracing::error!(error = %err, "Failed to get the certificate");
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

#[cfg(test)]
mod static_asset_tests {
    use super::{is_rewritable_text, rewrite_dashboard_paths};

    #[test]
    fn rewrite_covers_all_reference_forms() {
        let src = r#"<a href="/admin-dashboard/x"><img src="/admin-dashboard/y">"/admin-dashboard/z" '/admin-dashboard/w' (/admin-dashboard/q) /admin-dashboard/end"#;
        let out = String::from_utf8(rewrite_dashboard_paths(src, "/node")).unwrap();
        // Every /admin-dashboard reference is now prefixed with /node.
        assert!(!out.contains("\"/admin-dashboard"));
        assert!(!out.contains("'/admin-dashboard"));
        assert!(!out.contains("(/admin-dashboard"));
        assert!(!out.contains(" /admin-dashboard"));
        assert!(out.contains("href=\"/node/admin-dashboard/x"));
        assert!(out.contains("src=\"/node/admin-dashboard/y"));
        assert!(out.contains("\"/node/admin-dashboard/z"));
        assert!(out.contains("'/node/admin-dashboard/w"));
        assert!(out.contains("(/node/admin-dashboard/q"));
    }

    #[test]
    fn only_text_assets_are_rewritable() {
        assert!(is_rewritable_text("text/html"));
        assert!(is_rewritable_text("text/html; charset=utf-8"));
        assert!(is_rewritable_text("application/javascript"));
        assert!(is_rewritable_text("text/css"));
        assert!(!is_rewritable_text("image/png"));
        assert!(!is_rewritable_text("application/wasm"));
    }
}

#[cfg(test)]
mod parse_api_error_tests {
    use axum::http::StatusCode;

    use super::{parse_api_error, ApiError};

    #[test]
    fn not_a_group_member_maps_to_403_with_message() {
        let err = calimero_context::error::ContextError::NotAGroupMember {
            group_id: "test-group".to_owned(),
        };
        let api = parse_api_error(err.into());
        assert_eq!(api.status_code, StatusCode::FORBIDDEN);
        assert!(
            api.message.contains("not a member"),
            "expected the typed reason to reach the client, got: {}",
            api.message
        );
    }

    /// The whole point of the typed variant: a caller that got the init params
    /// wrong must be told so. Before this, the same case answered
    /// `500 {"error":"Internal server error"}` and the reason lived only in the
    /// node's log.
    #[test]
    fn init_failure_maps_to_400_carrying_the_guest_message() {
        let err = calimero_context::error::ContextError::InitFailed {
            message: "guest panicked: init: failed to deserialize arguments: \
                      missing field `name`"
                .to_owned(),
        };
        let api = parse_api_error(err.into());
        assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
        assert!(
            api.message.contains("missing field `name`"),
            "the guest's own diagnosis is the only useful part; got: {}",
            api.message
        );
    }

    /// "Wait" and "no" must not answer the same. A node that has not yet folded
    /// the history (or received a key it is entitled to) cannot decide the op's
    /// authority, and the apply burns nothing — so the identical call succeeds
    /// after it catches up. A caller told `500` cannot tell that from the
    /// permanent refusal next to it, and the two want opposite behaviour: retry
    /// versus stop.
    #[test]
    fn authority_not_yet_resolvable_maps_to_503_not_500() {
        let err = calimero_context::error::ContextError::AuthorityNotYetResolvable {
            group_id: "test-group".to_owned(),
        };
        let api = parse_api_error(err.into());
        assert_eq!(api.status_code, StatusCode::SERVICE_UNAVAILABLE);
        assert!(
            api.message.contains("retry"),
            "the client is being asked to come back, and the message should say so; \
             got: {}",
            api.message
        );
    }

    /// The pair that must stay apart: same shape of failure to a caller, opposite
    /// meanings. If these ever collapse to one status, retry logic built on the
    /// first will spin forever on the second.
    #[test]
    fn a_retryable_wait_and_a_permanent_refusal_get_different_statuses() {
        let waiting = parse_api_error(
            calimero_context::error::ContextError::AuthorityNotYetResolvable {
                group_id: "g".to_owned(),
            }
            .into(),
        );
        let refused = parse_api_error(
            calimero_context::error::ContextError::NotAGroupMember {
                group_id: "g".to_owned(),
            }
            .into(),
        );
        assert_ne!(
            waiting.status_code, refused.status_code,
            "'not yet' and 'never' must not be the same answer",
        );
        assert!(waiting.status_code.is_server_error());
        assert!(refused.status_code.is_client_error());
    }

    /// A refused delegated intent is the caller's problem, and the status has to
    /// say which kind. Before this mapping every refusal — an expired warrant, a
    /// warrant for another context, a relay with no authorship grant — arrived as
    /// `500`, so a client could not tell a malformed request from a broken node
    /// and had no basis for deciding whether to retry.
    #[test]
    fn an_intent_refusal_is_a_client_error_not_a_500() {
        use crate::admin::handlers::context::perform_intent::IntentRefusal;

        let malformed = parse_api_error(eyre::eyre!(IntentRefusal::Malformed(
            "delegation does not verify".to_owned()
        )));
        assert_eq!(malformed.status_code, StatusCode::BAD_REQUEST);

        let unauthorized = parse_api_error(eyre::eyre!(IntentRefusal::NotAuthorized(
            "an admin must grant CAN_AUTHOR_ON_BEHALF".to_owned()
        )));
        assert_eq!(unauthorized.status_code, StatusCode::FORBIDDEN);

        // The message survives, unlike the generic arm's. These are written for
        // the caller and say what to do next — which is the whole point of
        // typing them rather than letting them fall through.
        assert!(
            unauthorized.message.contains("CAN_AUTHOR_ON_BEHALF"),
            "the refusal should name the missing grant; got: {}",
            unauthorized.message
        );
    }

    /// "Your bytes are wrong" and "you are not allowed" must not collapse into
    /// one answer: the first is fixed by re-minting a warrant, the second only
    /// by an admin granting a capability. A client that cannot distinguish them
    /// either retries something that can never work, or gives up on something a
    /// grant would fix.
    #[test]
    fn malformed_and_unauthorized_intents_do_not_share_a_status() {
        use crate::admin::handlers::context::perform_intent::IntentRefusal;

        let malformed =
            parse_api_error(eyre::eyre!(IntentRefusal::Malformed("bad hex".to_owned())));
        let unauthorized = parse_api_error(eyre::eyre!(IntentRefusal::NotAuthorized(
            "no grant".to_owned()
        )));

        assert_ne!(malformed.status_code, unauthorized.status_code);
        assert!(malformed.status_code.is_client_error());
        assert!(unauthorized.status_code.is_client_error());
    }

    /// A refused warrant keeps its status through the wrapper the handler adds.
    ///
    /// This is the assumption the mapping rests on, and it is not obvious: the
    /// execute call is wrapped with `wrap_err("execution failed")`, so the
    /// `WarrantRefusal` is a *cause* rather than the report's root. If
    /// `downcast_ref` did not walk the chain, every replayed warrant would come
    /// back as a `500` and the mapping above would be dead code that looks
    /// alive. Asserting it here is cheaper than discovering it from a log.
    #[test]
    fn a_refused_warrant_keeps_its_status_through_the_execute_wrapper() {
        use eyre::WrapErr as _;

        let wrapped: eyre::Report = Err::<(), _>(
            calimero_governance_store::warrant_gate::WarrantRefusal::NonceAlreadySpent,
        )
        .wrap_err("execution failed")
        .expect_err("must be an error");

        let api = parse_api_error(wrapped);
        assert_eq!(
            api.status_code,
            StatusCode::FORBIDDEN,
            "a spent warrant is the caller's problem, not a server fault"
        );
        assert!(
            api.message.contains("already been spent"),
            "the caller should learn the warrant was spent; got: {}",
            api.message
        );
    }

    #[test]
    fn untyped_error_stays_a_generic_500() {
        let api = parse_api_error(eyre::eyre!("some internal detail with /store/path"));
        assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
        // The internal detail must not leak to the client.
        assert_eq!(api.message, "Internal server error");
    }

    #[test]
    fn typed_api_error_is_preserved() {
        let api = parse_api_error(
            ApiError {
                status_code: StatusCode::BAD_REQUEST,
                message: "bad group id".to_owned(),
            }
            .into(),
        );
        assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
        assert_eq!(api.message, "bad group id");
    }
}
