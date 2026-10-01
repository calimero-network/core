use core::error::Error;
use core::fmt::{self, Display, Formatter};
use std::str;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Response, StatusCode, Uri};
use axum::response::IntoResponse;
use axum::routing::{get, post, put};
use axum::{Extension, Router};
use bytes::Bytes;
use calimero_context_client::messages::ExecuteError;
use calimero_governance_store::{
    ApplyError, CapabilitiesError, ContextRegistrationError, GroupCreatedRejection,
    GroupDeletedRejection, MemberJoinedOpenRejection, MembershipError, MetaError, NamespaceError,
};
use eyre::Report;
use rust_embed::{EmbeddedFile, RustEmbed};
use serde::{Deserialize, Serialize};
use serde_json::{json, to_string as to_json_string};
use tower_sessions::{MemoryStore, SessionManagerLayer};
use tracing::info;

use super::handlers::{account, alias, blob, groups, namespaces, tee};
use crate::admin::handlers::applications::{
    get_application, get_application_abi, install_application, install_dev_application,
    list_application_versions, list_applications, uninstall_application,
};
use crate::admin::handlers::context::{
    create_context, create_context_intent, delete_context, get_context, get_context_group,
    get_context_identities, get_context_ids, get_context_storage, get_contexts_for_application,
    get_contexts_with_executors_for_application, governance_intent, intent_relay, join_context,
    leave_context, perform_intent, query_context, resync_context, sync, update_context_application,
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

    /// Serve the delegated-execution routes without a node credential.
    ///
    /// # What this opens, and what it does not
    ///
    /// It opens exactly two routes — `GET`/`POST
    /// /admin-api/contexts/:context_id/intents` — and nothing else. Both are
    /// self-authenticating: the `POST` carries a warrant signed by the author's
    /// device key that commits to this context, this method and these arguments,
    /// has not expired, and whose nonce this node has not spent; and it is
    /// refused unless this node holds `CAN_AUTHOR_ON_BEHALF` on the group owning
    /// the context. A node token proves none of that and none of that needs a
    /// node token.
    ///
    /// # Why it is off by default and why it exists at all
    ///
    /// Off, because a self-hosted node's operator did not ask for an
    /// unauthenticated surface and the routes are useless to them: a member with
    /// a node authors its own writes.
    ///
    /// On, because the clients delegated authorship exists for — a browser tab,
    /// a phone, an agent holding one signing key — have no relationship with the
    /// relay and so cannot hold a credential on it. Requiring one makes the
    /// feature unreachable for precisely the callers it was built for, which is
    /// recorded as the known gap in the direct-admission docs. Hosted TEE relays
    /// set this; nothing else should need to.
    ///
    /// The residual exposure is warrant verification on an unauthenticated
    /// request: signature checks and one store read, refused before anything is
    /// executed or published. Rate limiting that hop belongs to the deployment's
    /// reverse proxy, as it does for the other public routes.
    ///
    /// # What it does under each auth mode — read this before relying on it
    ///
    /// Under [`AuthMode::Embedded`](crate::config::AuthMode::Embedded) this is
    /// the gate: the guard layer wraps the protected router only, so moving the
    /// routes across genuinely removes the credential requirement.
    ///
    /// Under [`AuthMode::Proxy`](crate::config::AuthMode::Proxy) — the default —
    /// **core installs no guard at all**, on either router. A reverse proxy
    /// enforces auth, and it is the only thing that does. So flipping this alone
    /// changes nothing a caller can observe, and *not* flipping it does not
    /// close the path: whatever the proxy exempts is open regardless.
    ///
    /// That asymmetry is the trap. A proxy deployment must set this **and**
    /// exempt exactly this path at the ingress, from one source of truth, or the
    /// two drift — and the drift is silent in the unsafe direction, because the
    /// ingress exemption is the half that actually opens the door. mero-tee's
    /// node image drives both from a single Ansible variable for this reason.
    /// Reads `public_intents` too, for one release.
    ///
    /// The flag and the config key move in OPPOSITE directions, and only one of
    /// them is safe to get wrong. A new merod reading an old `config.toml` is
    /// covered by this alias. An OLD merod reading a NEW key is not, and cannot
    /// be: it simply does not know `delegated_access`, so `#[serde(default)]`
    /// leaves it false and the relay comes up with the surface silently SHUT.
    /// That is why the fleet's node image keeps writing the old key until its
    /// merod is past this release — a rollback that reads as "the flag stopped
    /// working" rather than as a parse error.
    #[serde(default, alias = "public_intents")]
    pub delegated_access: bool,
}

impl AdminConfig {
    /// One constructor taking both flags, rather than a `new(enabled)` plus a
    /// `with_delegated_access`.
    ///
    /// `delegated_access` decides whether this node exposes a write path to
    /// callers holding no credential on it, so there should be no way to build
    /// this type without answering that. A default-false convenience
    /// constructor is exactly how a relay ships with the posture nobody
    /// intended — in either direction.
    #[must_use]
    pub const fn new(enabled: bool, delegated_access: bool) -> Self {
        Self {
            enabled,
            delegated_access,
        }
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
    let admin_config = match &config.admin {
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
            "/applications/{application_id}",
            get(get_application::handler).delete(uninstall_application::handler),
        )
        .route(
            "/applications/{application_id}/abi",
            get(get_application_abi::handler),
        )
        .route(
            "/applications/{application_id}/versions",
            get(list_application_versions::handler),
        )
        // Package management
        .route("/packages", get(list_packages::handler))
        .route("/packages/{package}/versions", get(list_versions::handler))
        .route("/packages/{package}/latest", get(get_latest_version::handler))
        // Context management
        .route(
            "/contexts",
            get(get_context_ids::handler).post(create_context::handler),
        )
        .route(
            "/contexts/{context_id}",
            get(get_context::handler).delete(delete_context::handler),
        )
        .route(
            "/contexts/{context_id}/application",
            post(update_context_application::handler),
        )
        .route(
            "/contexts/{context_id}/resync",
            post(resync_context::handler),
        )
        .route(
            "/contexts/for-application/{application_id}",
            get(get_contexts_for_application::handler),
        )
        .route(
            "/contexts/with-executors/for-application/{application_id}",
            get(get_contexts_with_executors_for_application::handler),
        )
        .route(
            "/contexts/{context_id}/storage",
            get(get_context_storage::handler),
        )
        .route(
            "/contexts/{context_id}/identities",
            get(get_context_identities::handler),
        )
        .route(
            "/contexts/{context_id}/identities-owned",
            get(get_context_identities::handler),
        )
        .route(
            "/contexts/{context_id}/group",
            get(get_context_group::handler),
        )
        // A delegated read: an account-authenticated caller reads a context it
        // is a member of. Protected rather than public, unlike the intent pair
        // — an intent carries a warrant that stands on its own, while this
        // carries only a session, so the auth layer is what identifies the
        // caller at all.
        .route(
            "/contexts/{context_id}/query",
            post(query_context::handler),
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
                .route("/{context_id}", post(sync::handler)),
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
            "/blobs/{blob_id}",
            get(blob::download_handler)
                .head(blob::info_handler)
                .delete(blob::delete_handler),
        )
        // Group management
        .route("/groups", post(groups::create_group::handler))
        .route(
            "/groups/{group_id}",
            get(groups::get_group_info::handler).delete(groups::delete_group::handler),
        )
        .route(
            "/groups/{group_id}/contexts",
            get(groups::list_group_contexts::handler),
        )
        .route(
            "/groups/{group_id}/reparent",
            post(groups::reparent_group::handler),
        )
        .route(
            "/groups/{group_id}/subgroups",
            get(groups::list_subgroups::handler),
        )
        .route(
            "/groups/{group_id}/members",
            get(groups::list_group_members::handler).post(groups::add_group_members::handler),
        )
        .route(
            "/groups/{group_id}/members/remove",
            post(groups::remove_group_members::handler),
        )
        .route(
            "/groups/{group_id}/member-devices",
            get(groups::list_member_devices::handler),
        )
        .route(
            "/groups/{group_id}/accounts/{account}/seal",
            post(groups::seal_to_account::handler),
        )
        .route(
            "/groups/{group_id}/leave",
            post(groups::leave_group::handler),
        )
        // Owner-level ops: each needs the owner account's root proof, not only
        // one of its devices (`calimero_governance_store::owner_guard`).
        .route(
            "/groups/{group_id}/transfer-ownership",
            post(groups::transfer_ownership::handler),
        )
        .route(
            "/groups/{group_id}/owner-delete",
            post(groups::owner_delete_group::handler),
        )
        .route(
            "/groups/{group_id}/members/{account}/role",
            put(groups::update_member_role::handler),
        )
        .route(
            "/groups/{group_id}/metadata",
            get(groups::set_group_metadata::get_handler).put(groups::set_group_metadata::handler),
        )
        .route(
            "/groups/{group_id}/members/{account}/metadata",
            get(groups::set_member_metadata::get_handler).put(groups::set_member_metadata::handler),
        )
        .route(
            "/groups/{group_id}/contexts/{context_id}/metadata",
            get(groups::set_context_metadata::get_handler)
                .put(groups::set_context_metadata::handler),
        )
        .route(
            "/groups/{group_id}/contexts/{context_id}/remove",
            post(groups::detach_context_from_group::handler),
        )
        .route(
            "/groups/{group_id}/upgrade",
            post(groups::upgrade_group::handler),
        )
        .route(
            "/groups/{group_id}/upgrade/status",
            get(groups::get_group_upgrade_status::handler),
        )
        .route(
            "/groups/{namespace_id}/cascade-status",
            get(groups::get_cascade_status::handler),
        )
        .route(
            "/groups/{namespace_id}/migration-status",
            get(groups::get_migration_status::handler),
        )
        .route(
            "/groups/{namespace_id}/migration/abort",
            post(groups::abort_migration::handler),
        )
        .route(
            "/groups/{group_id}/upgrade/retry",
            post(groups::retry_group_upgrade::handler),
        )
        .route(
            "/groups/{group_id}/issue-ownership-proof",
            post(groups::issue_ownership_proof::handler),
        )
        .route(
            "/groups/{group_id}/issue-namespace-ownership-proof",
            post(groups::issue_namespace_ownership_proof::handler),
        )
        .route(
            "/groups/{group_id}/sync",
            post(groups::sync_group::handler),
        )
        .route(
            "/groups/{group_id}/join-via-inheritance",
            post(groups::join_subgroup_inheritance::handler),
        )
        .route(
            "/contexts/{context_id}/join",
            post(join_context::handler),
        )
        .route(
            "/contexts/{context_id}/leave",
            post(leave_context::handler),
        )
        .route(
            "/groups/{group_id}/members/{account}/capabilities",
            get(groups::get_member_capabilities::handler)
                .put(groups::set_member_capabilities::handler),
        )
        .route(
            "/groups/{group_id}/members/{account}/auto-follow",
            put(groups::set_member_auto_follow::handler),
        )
        .route(
            "/groups/{group_id}/settings/default-capabilities",
            put(groups::set_default_capabilities::handler),
        )
        .route(
            "/groups/{group_id}/settings/tee-admission-policy",
            get(groups::get_tee_admission_policy::handler)
                .put(groups::set_tee_admission_policy::handler),
        )
        .route(
            "/groups/{group_id}/settings/tee-authoring-policy",
            put(groups::set_tee_authoring_policy::handler)
                .delete(groups::disable_tee_authoring_policy::handler),
        )
        .route(
            "/groups/{group_id}/settings/subgroup-visibility",
            put(groups::set_subgroup_visibility::handler),
        )
        // Legacy subgroup invitation/join routes kept for backwards compatibility.
        .route(
            "/groups/{group_id}/invite",
            post(groups::create_group_invitation::handler),
        )
        .route("/groups/join", post(groups::join_group::handler))
        // Pairing is account-level: the certificate is root-signed and the
        // endorsement node-level, so neither half ever named a namespace.
        .route("/account/pair-init", post(account::pair_init::handler))
        .route(
            "/account/sign-with-root",
            post(account::sign_with_root::handler),
        )
        .route(
            "/account/pair-complete",
            post(account::pair_complete::handler),
        )
        // Read-only aggregation over the same account-level state: the settings
        // UI's device list and the applications this account speaks in.
        .route("/account/devices", get(account::devices::handler))
        .route(
            "/account/applications",
            get(account::applications::handler),
        )
        // Pairing is a snapshot; this is how it is repeated. A namespace gained
        // after a pairing binds its devices on its own, and this closes the drift
        // left by every one gained before that landed.
        .route(
            "/account/devices/{device_id}/relink",
            post(account::relink::handler),
        )
        // The other direction, which relink deliberately cannot do: replace the
        // scope outright, so an application can be taken away again.
        .route(
            "/account/devices/{device_id}/scope",
            put(account::rescope::handler),
        )
        // The name every device of the account renders, as opposed to whatever
        // alias one node happens to hold locally.
        .route(
            "/account/devices/{device_id}/label",
            put(account::label::handler),
        )
        .route(
            "/namespaces/{namespace_id}/account/revoke",
            post(namespaces::revoke_device::handler),
        )
        .route(
            "/namespaces/{namespace_id}/admin",
            post(namespaces::change_admin::handler),
        )
        // The relay half of a nodeless account minting invitations: bind a device
        // this node does not hold, so what it signs resolves to its account.
        .route(
            "/namespaces/{namespace_id}/account/link-device",
            post(namespaces::link_device::handler),
        )
        // Namespace management
        .route(
            "/namespaces",
            get(namespaces::list::handler).post(namespaces::create_namespace::handler),
        )
        .route(
            "/namespaces/{namespace_id}",
            get(namespaces::get_namespace::handler).delete(namespaces::delete_namespace::handler),
        )
        .route(
            "/namespaces/{namespace_id}/invite",
            post(namespaces::invite_namespace::handler),
        )
        .route(
            "/namespaces/{namespace_id}/join",
            post(namespaces::join_namespace::handler),
        )
        .route(
            "/namespaces/{namespace_id}/admit",
            post(namespaces::admit_join::handler),

        )
        .route(
            "/namespaces/{namespace_id}/leave",
            post(namespaces::leave_namespace::handler),
        )
        .route(
            "/namespaces/for-application/{application_id}",
            get(namespaces::list_for_application::handler),
        )
        // Namespace governance (Phase 2)
        .route(
            "/namespaces/{namespace_id}/groups",
            get(namespaces::list_namespace_groups::handler)
                .post(namespaces::create_group_in_namespace::handler),
        )
        // TEE protected endpoints
        .nest("/tee", tee::protected_service())
        // Alias management
        .nest("/alias", alias::service())
        // Delegated execution, unless this deployment serves it publicly. The
        // routes are built once and mounted on exactly one of the two routers,
        // so the two postures cannot both be live and no ordering between the
        // routers decides which wins.
        .merge(if admin_config.delegated_access {
            Router::new()
        } else {
            delegated_execution_routes()
        })
        .layer(Extension(Arc::clone(&shared_state)))
        .layer(session_layer.clone());

    let public_routes = Router::new()
        .route("/health", get(health_check_handler))
        .route("/ready", get(readiness_check_handler))
        .route("/is-authed", get(is_authed_handler))
        .nest("/tee", tee::service())
        .merge(if admin_config.delegated_access {
            info!(
                "Delegated execution is served publicly: a warrant is the credential on \
                 GET/POST {admin_path}/contexts/:context_id/intents and \
                 GET/POST {admin_path}/groups/:group_id/context-intents and \
                 GET/POST {admin_path}/groups/:group_id/governance-intents"
            );
            delegated_execution_routes()
        } else {
            Router::new()
        })
        .layer(Extension(shared_state));

    Some((admin_path, protected_routes, public_routes))
}

/// The delegated-execution surface: run one intent a member authorized, and read
/// what a member needs to know before authorizing one.
///
/// One function rather than two route lines in each router, because the two
/// halves have to move together. A client that can `POST` but not `GET` cannot
/// learn which account to name as the executor, and a client that can `GET` but
/// not `POST` learns the answer to a question it cannot then act on — either
/// split is a surface that looks available and is not.
///
/// Which router this is merged into is [`AdminConfig::delegated_access`]; see there
/// for why an unauthenticated posture is a coherent choice for these two routes
/// and only these two.
fn delegated_execution_routes() -> Router {
    Router::new()
        .route(
            "/contexts/{context_id}/intents",
            post(perform_intent::handler).get(intent_relay::handler),
        )
        // Creating the context a member's later intents run in. On the same
        // router as the intents, for the reason the pair above is one function:
        // a member who can write through a relay but not create through it can
        // use no context it did not already have.
        .route(
            "/groups/{group_id}/context-intents",
            post(create_context_intent::handler).get(create_context_intent::describe_handler),
        )
        // And the governance around it: adding the other person to a DM,
        // creating a channel's subgroup, renaming it.
        .route(
            "/groups/{group_id}/governance-intents",
            post(governance_intent::handler).get(governance_intent::describe_handler),
        )
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
        .route("/{*path}", get(serve_embedded_file)) // Match all sub-paths
        .layer(axum::middleware::map_response(with_dashboard_security_headers));

    Some((path, router))
}

const DASHBOARD_SECURITY_HEADERS: [(&str, &str); 4] = [
    (
        "content-security-policy",
        "frame-ancestors 'none'; object-src 'none'; base-uri 'self'",
    ),
    ("x-frame-options", "DENY"),
    ("x-content-type-options", "nosniff"),
    ("referrer-policy", "no-referrer"),
];

async fn with_dashboard_security_headers(mut response: Response<Body>) -> Response<Body> {
    apply_dashboard_security_headers(response.headers_mut());
    response
}

fn apply_dashboard_security_headers(headers: &mut axum::http::HeaderMap) {
    for (name, value) in DASHBOARD_SECURITY_HEADERS {
        let _previous = headers.insert(name, axum::http::HeaderValue::from_static(value));
    }
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

/// `NODE_PATH_PREFIX`, resolved once per process. `None` when unset (the
/// overwhelmingly common case), which lets [`serve_file`] skip the dashboard
/// base-path rewrite (and its full-content `.replace()` passes) entirely.
pub(crate) fn node_path_prefix() -> Option<&'static str> {
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
    PREFIX.get_or_init(|| match node_path_prefix() {
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

    let body = match (node_path_prefix(), is_rewritable_text(&mimetype)) {
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

impl ApiError {
    /// True when the status blames the caller rather than this node.
    ///
    /// Admin handlers log a failed call at `error!`. On a fleet node those
    /// journals ship to a central log store, so a UI polling an absent group
    /// every 1.4s turned a routine `404` into a permanent ERROR stream --
    /// noise in exactly the place someone looks when diagnosing a real fault.
    /// A handler that can legitimately answer 4xx picks its level with this.
    pub(crate) fn is_client_fault(&self) -> bool {
        self.status_code.is_client_error()
    }
}

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

/// The status a device-pairing refusal answers with, or `None` if `err` is not
/// one.
///
/// Grouped rather than one arm apiece because the statuses are the whole
/// distinction a client can act on: fix the payload, come back when this node is
/// ready, go to the node that holds the account, or stop - this can never work.
fn pairing_refusal_status(err: &calimero_context::error::ContextError) -> Option<StatusCode> {
    use calimero_context::error::ContextError as Refusal;

    Some(match err {
        Refusal::PairingStatementInvalid { .. }
        | Refusal::PairingCodeMismatch { .. }
        | Refusal::ScopeReplacementEmpty
        | Refusal::ScopeReplacementTooLarge { .. }
        | Refusal::ScopeReplacementUnknownApplication { .. }
        | Refusal::DeviceLabelInvalid { .. }
        | Refusal::DeviceLinkInvalid { .. } => StatusCode::BAD_REQUEST,
        Refusal::PairingNoNamespaceIdentity { .. }
        | Refusal::PairingNoScopeKey { .. }
        | Refusal::ScopeEpochExhausted { .. }
        | Refusal::DeviceLabelUnavailable { .. } => StatusCode::CONFLICT,
        Refusal::PairingNotTheAccountHolder { .. }
        | Refusal::PairingDeviceRevoked { .. }
        | Refusal::ScopeReplacementHoldsTheRoot { .. }
        | Refusal::DeviceLabelNotOwn { .. }
        | Refusal::RevocationOfOwnDevice { .. }
        | Refusal::DeviceLinkRefused { .. } => StatusCode::FORBIDDEN,
        Refusal::DeviceRenamedTooRecently { .. } => StatusCode::TOO_MANY_REQUESTS,
        Refusal::PairingUnknownDevice { .. } | Refusal::RevocationUnknownDevice { .. } => {
            StatusCode::NOT_FOUND
        }
        _ => return None,
    })
}

/// The status an upgrade or leave refusal answers with, or `None` for any other
/// error. Each is a precondition the caller can read and act on (wait, install a
/// new version, use the namespace leave), which a `500` would hide.
fn group_lifecycle_refusal_status(
    err: &calimero_context::error::ContextError,
) -> Option<StatusCode> {
    use calimero_context::error::ContextError as Refusal;

    Some(match err {
        Refusal::LeaveGroupIsNamespace { .. }
        | Refusal::InvitationInvalid { .. }
        | Refusal::TeePolicyInvalid { .. }
        | Refusal::OwnershipProofInvalid { .. }
        | Refusal::BytecodeIdInvalid { .. } => StatusCode::BAD_REQUEST,
        Refusal::UpgradeNotFound { .. } => StatusCode::NOT_FOUND,
        Refusal::UpgradeInProgress { .. }
        | Refusal::LeaveGroupNotDirectMember { .. }
        | Refusal::UpgradeAlreadyTargeting { .. }
        | Refusal::UpgradeNoContexts { .. }
        | Refusal::UpgradeNotRetryable { .. }
        | Refusal::UpgradeRefused { .. }
        | Refusal::InvitationExpired { .. }
        | Refusal::GroupAlreadyExists { .. }
        | Refusal::ContextSeedCollision
        | Refusal::ResyncRefused { .. } => StatusCode::CONFLICT,
        // Retryable: the join went out and the key is on its way. The same
        // answer a context call gets while its group key is pending.
        Refusal::JoinKeyDeliveryTimedOut { .. } => StatusCode::SERVICE_UNAVAILABLE,
        _ => return None,
    })
}

/// The status a membership refusal answers with, or `None` if it is this node's
/// fault rather than the caller's.
///
/// Every one of these was a `500 {"error":"Internal server error"}` before, and
/// that is the wrong answer to all but the last group: the gate did its job, the
/// request was understood and deliberately refused. A client told `500` cannot
/// tell "you are not allowed" from "the node fell over", so it can neither
/// explain the refusal to a person nor decide whether a retry is pointless.
///
/// Matched exhaustively on purpose: a new variant must be classified here rather
/// than silently inheriting the generic 500 through a `_` arm.
fn membership_refusal_status(err: &MembershipError) -> Option<StatusCode> {
    use MembershipError as Refusal;

    Some(match err {
        // The caller lacks the authority, and no change of state on their part
        // will help — only being granted it will.
        Refusal::NotAdmin { .. }
        | Refusal::NotMember { .. }
        | Refusal::SelfLeaveOnly
        | Refusal::AutoFollowAuthFailed
        | Refusal::OnlyOwnerCanTransfer(_)
        | Refusal::OnlyOwnerCanDelete(_)
        | Refusal::OnlyOwnerCanChangeAdmin(_)
        | Refusal::OwnerImmuneFromRemoval(_)
        | Refusal::OwnerCannotSelfLeave(_)
        | Refusal::TeeVerifierNotAuthorized
        | Refusal::TeeVaultKeyNotFromTee
        | Refusal::TeeRoleViaAttestationOnly
        | Refusal::TeeRoleNotPolicyMode { .. }
        | Refusal::TeeMemberRoleLocked { .. }
        | Refusal::TeeAdmissionWrongNamespace { .. }
        | Refusal::TeeCredentialNotTheAttestedKey { .. } => StatusCode::FORBIDDEN,

        // Well-formed and permitted, but it conflicts with how the group looks
        // right now. Escalating privileges does not help; changing the group
        // does — transfer ownership first, issue a fresh invitation, re-add the
        // member.
        Refusal::LastAdmin
        | Refusal::LastAdminDemotion
        | Refusal::RemovedFromGroup { .. }
        | Refusal::ReentryBlocked { .. }
        | Refusal::InvitationAlreadyConsumed { .. }
        | Refusal::OwnerOwnsSubgroup(_)
        | Refusal::TransferTargetNotAdmin { .. }
        | Refusal::TransferTargetNotMember(_)
        | Refusal::MemberNotDirect(_)
        | Refusal::NoTeeAdmissionPolicy => StatusCode::CONFLICT,

        Refusal::UnknownGroup(_) | Refusal::MemberNotFound { .. } => StatusCode::NOT_FOUND,

        // Genuinely this node's problem: a row that exists with no value, or a
        // parent chain that will not terminate. Fall through to the generic 500,
        // which also keeps their messages (they name internal rows) out of the
        // response.
        // `TeeAdmissionPolicyUnreadable` joins them: the op log holds bytes
        // this binary cannot decode, which is not something the requester did
        // or can undo. The reason is logged at `error!` where it is actionable;
        // the response stays generic, like its neighbours here.
        Refusal::MissingMemberValue { .. }
        | Refusal::DepthExceeded(_)
        | Refusal::TeeAdmissionPolicyUnreadable(_) => return None,
    })
}

/// The status a namespace-tree refusal answers with, or `None` when it is this
/// node's fault. Matched exhaustively, like [`membership_refusal_status`], so a
/// new variant has to be classified rather than inheriting the generic 500.
fn namespace_refusal_status(err: &NamespaceError) -> Option<StatusCode> {
    use NamespaceError as Refusal;

    Some(match err {
        // Asking for a tree shape that can never be valid.
        Refusal::SelfNesting
        | Refusal::RootHasNoParent(_)
        | Refusal::ReparentCrossNamespace { .. }
        | Refusal::GroupOutsideNamespace { .. }
        | Refusal::CannotDeleteRoot(_)
        | Refusal::SelfParentEdge
        | Refusal::TeePolicyNotOnSubgroup(_)
        | Refusal::TeeAuthoringPolicyNotOnSubgroup(_) => StatusCode::BAD_REQUEST,
        // Valid in general, but not against the tree as it stands.
        Refusal::NestingCycle | Refusal::AlreadyHasParent(_) | Refusal::ReparentCycle { .. } => {
            StatusCode::CONFLICT
        }
        Refusal::ReparentTargetMissing(_) => StatusCode::NOT_FOUND,
        Refusal::NoNamespaceIdentity(_) | Refusal::ReadOnlyTee => StatusCode::FORBIDDEN,
        // A tree too deep to walk, or a namespace with no root row: the store,
        // not the request.
        Refusal::DepthExceeded | Refusal::RootMissing => return None,
    })
}

/// The status a governance-op apply refusal answers with, or `None` when it is
/// this node's fault.
///
/// The `*Rejected` wrappers are matched here, on the outer type, because a
/// report's downcast does not follow `#[source]` into the rejection inside.
fn apply_refusal_status(err: &ApplyError) -> Option<StatusCode> {
    Some(match err {
        ApplyError::GroupCreatedRejected(GroupCreatedRejection::Unauthorized { .. })
        | ApplyError::GroupDeletedRejected(GroupDeletedRejection::Unauthorized { .. })
        | ApplyError::MemberJoinedOpenRejected(MemberJoinedOpenRejection::NoMembershipPath {
            ..
        }) => StatusCode::FORBIDDEN,
        ApplyError::GroupCreatedRejected(
            GroupCreatedRejection::ParentCrossNamespace { .. }
            | GroupCreatedRejection::GroupIdNotDerived { .. },
        ) => StatusCode::BAD_REQUEST,
        ApplyError::GroupDeletedRejected(
            GroupDeletedRejection::CascadeDivergenceGroups { .. }
            | GroupDeletedRejection::CascadeDivergenceContexts { .. },
        )
        | ApplyError::GroupCreatedRejected(
            GroupCreatedRejection::ExistingGroupNotOwned { .. }
            | GroupCreatedRejection::ExistingGroupParentMismatch { .. }
            | GroupCreatedRejection::ParentIsDescendant { .. }
            | GroupCreatedRejection::ExistingGroupIsNamespaceRoot { .. },
        )
        | ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::ReentryBlocked { .. }
            | MemberJoinedOpenRejection::AlreadyDirectMember(_),
        )
        | ApplyError::StateHashMismatch { .. }
        | ApplyError::StaleNonce { .. } => StatusCode::CONFLICT,
        // Not a refusal: the node lacks the history to decide yet, and the same
        // call succeeds once it has caught up. Same answer as
        // `AuthorityNotYetResolvable` below.
        ApplyError::AuthorityUndecidable { .. } | ApplyError::DagHeadsExceeded => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        // Ops this node signed itself and got wrong: its fault, not the caller's.
        ApplyError::UnsupportedOp
        | ApplyError::NonceOverflow
        | ApplyError::MemberJoinedOpenRejected(
            MemberJoinedOpenRejection::SignerMismatch { .. }
            | MemberJoinedOpenRejection::WrongNamespace { .. },
        )
        | ApplyError::NamespaceCreatedRejected(_) => return None,
    })
}

/// The status an execution failure answers with, or `None` for an internal one.
///
/// The execute path already sorts its failures into these variants; without
/// this they all reached the caller as one `500`, so "that context does not
/// exist" and "try again once the group key arrives" looked the same as a crash.
fn execute_refusal_status(err: &ExecuteError) -> Option<StatusCode> {
    Some(match err {
        ExecuteError::ContextNotFound => StatusCode::NOT_FOUND,
        ExecuteError::Unauthorized { .. }
        | ExecuteError::XCallNotPermitted { .. }
        | ExecuteError::NotAMember { .. }
        // A delegated write refused over a role — a TEE replica asked to relay,
        // or a read-only author — before it ran. Authority is missing; the
        // request itself is fine.
        | ExecuteError::DelegatedWriteRefused { .. } => StatusCode::FORBIDDEN,
        // A write during a cascade upgrade, or a write on a read-only session:
        // the call conflicts with the context's current state or the session's
        // scope, which the caller has to change.
        ExecuteError::UpgradeInProgress { .. } | ExecuteError::NotReadOnly { .. } => {
            StatusCode::CONFLICT
        }
        // The node is still catching up: state sync, the group key, or the
        // application bytecode. The identical call succeeds later.
        ExecuteError::Uninitialized
        | ExecuteError::GroupKeyPending { .. }
        | ExecuteError::ApplicationNotInstalled { .. } => StatusCode::SERVICE_UNAVAILABLE,
        _ => return None,
    })
}

#[must_use]
/// The status a root-guard refusal answers with. Every variant is the caller's:
/// a proof that is missing, malformed, for something else, or already spent.
fn owner_guard_status(refusal: &calimero_governance_store::OwnerGuardRefusal) -> StatusCode {
    use calimero_governance_store::OwnerGuardRefusal as Refusal;
    match refusal {
        // Only the root holder can help, so it is about standing.
        Refusal::ProofRequired { .. }
        | Refusal::SignerUnbound
        | Refusal::ProofAccountMismatch { .. } => StatusCode::FORBIDDEN,
        // The group moved on: read the counter again and re-sign.
        Refusal::StaleCounter { .. } => StatusCode::CONFLICT,
        Refusal::NotAGuardedKind { .. }
        | Refusal::ProofInvalid(_)
        | Refusal::ProofMismatch { .. }
        | Refusal::BelowRecordedEpoch { .. }
        | Refusal::ForkedChain { .. } => StatusCode::BAD_REQUEST,
    }
}

pub fn parse_api_error(err: Report) -> ApiError {
    // A root-guard refusal: the owner-level op's proof, or its absence.
    if let Some(refusal) = err.downcast_ref::<calimero_governance_store::OwnerGuardRefusal>() {
        return ApiError {
            status_code: owner_guard_status(refusal),
            message: format!("{err:#}"),
        };
    }
    // A membership refusal: the governance gate understood the request and said
    // no. Which "no" it is decides what the caller should do next, so map it
    // rather than flattening the whole family into the generic 500 below.
    if let Some(status_code) = err
        .downcast_ref::<MembershipError>()
        .and_then(membership_refusal_status)
    {
        return ApiError {
            status_code,
            // The whole chain: a handler that wrapped the refusal for context
            // (the join relay does) would otherwise hide the refusal itself.
            message: format!("{err:#}"),
        };
    }
    // A membership-gate rejection ("node is not a member of group X") is a
    // legitimate client-side precondition, not a server fault. Surface it as a
    // typed 403 with its (safe, intended) message instead of letting it fall
    // through to the generic 500 below. This is what a caller sees when it
    // lists a group the node hasn't joined / isn't in.
    // `DeviceOutOfScope` rides along: it is the same kind of "no" about this
    // node's own standing, and a `500` would read as a server fault.
    if let Some(
        calimero_context::error::ContextError::NotAGroupMember { .. }
        | calimero_context::error::ContextError::NotANamespaceMember { .. }
        // A caller-supplied identity without standing in the group. 403 like
        // its neighbours, and never 404: the caller holds this key and is
        // acting AS this identity, so the refusal is about standing rather
        // than about something being absent.
        | calimero_context::error::ContextError::IdentityNotAGroupMember { .. }
        | calimero_context::error::ContextError::NotAGroupAdmin { .. }
        | calimero_context::error::ContextError::SubgroupCreationNeedsNamespaceAdmin { .. }
        | calimero_context::error::ContextError::CallerNotPermitted
        | calimero_context::error::ContextError::DeviceOutOfScope { .. }
        | calimero_context::error::ContextError::RootProofRequired { .. },
    ) = err.downcast_ref::<calimero_context::error::ContextError>()
    {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: err.to_string(),
        };
    }
    // The caller named something this node does not have. `404` rather than
    // the generic `500`: the two ask opposite things of a client, and a
    // control-plane script reading a `500` as "already gone" is how a real
    // failure got walked past during the fleet-HA incident. These messages
    // carry only the id the caller supplied, so echoing them leaks nothing.
    if let Some(
        calimero_context::error::ContextError::GroupNotFound { .. }
        | calimero_context::error::ContextError::NamespaceNotFound { .. }
        | calimero_context::error::ContextError::ApplicationNotFound { .. }
        | calimero_context::error::ContextError::ContextNotFound { .. }
        | calimero_context::error::ContextError::BytecodeNotInstalled { .. },
    ) = err.downcast_ref::<calimero_context::error::ContextError>()
    {
        return ApiError {
            status_code: StatusCode::NOT_FOUND,
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
    // A delegated creation the registration gate refused. Like a warrant
    // refusal, every variant is about the member's or the relay's authority, or
    // a relay rewriting what was signed — never this node's health.
    if let Some(refusal) =
        err.downcast_ref::<calimero_governance_store::delegation_gate::DelegationRefusal>()
    {
        use calimero_governance_store::delegation_gate::DelegationRefusal;
        return ApiError {
            status_code: if matches!(refusal, DelegationRefusal::GroupAlreadyExists(_)) {
                StatusCode::CONFLICT
            } else {
                StatusCode::FORBIDDEN
            },
            message: refusal.to_string(),
        };
    }
    if let Some(refusal) =
        err.downcast_ref::<calimero_governance_store::creation_gate::CreationRefusal>()
    {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: refusal.to_string(),
        };
    }
    // The member signed against an application the group no longer targets.
    // Nothing is wrong with the node or the signature; the member re-signs.
    if let Some(
        refused @ calimero_context::error::ContextError::DelegatedApplicationNotTargeted { .. },
    ) = err.downcast_ref::<calimero_context::error::ContextError>()
    {
        return ApiError {
            status_code: StatusCode::CONFLICT,
            message: refused.to_string(),
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
    // A join no admitter endorsed. Retryable either way, so never the generic
    // 500: `504` when no peer of the namespace answered at all, which is what a
    // client sees while the inviting node is offline, and `503` when one answered
    // but is not an admitter the invitation names.
    if let Some(calimero_context::error::ContextError::JoinNotEndorsed { reached_a_peer }) =
        err.downcast_ref::<calimero_context::error::ContextError>()
    {
        return ApiError {
            status_code: if *reached_a_peer {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::GATEWAY_TIMEOUT
            },
            message: format!("{err:#}"),
        };
    }
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
    // A pairing refusal is always about the caller or about this node's state,
    // never about its health, and the five are not interchangeable: retrying a
    // bad confirmation code is pointless, retrying a missing scope key is
    // exactly right. Flattened to one `500` a client can tell neither.
    if let Some(status_code) = err
        .downcast_ref::<calimero_context::error::ContextError>()
        .and_then(group_lifecycle_refusal_status)
    {
        return ApiError {
            status_code,
            message: err.to_string(),
        };
    }
    if let Some(status_code) = err
        .downcast_ref::<calimero_context::error::ContextError>()
        .and_then(pairing_refusal_status)
    {
        return ApiError {
            status_code,
            message: err.to_string(),
        };
    }
    // This node's own device slot already holds a linked device for another
    // account. The request is well-formed and the node is healthy; it is simply
    // not the machine that may certify here, which no retry changes.
    if let Some(refusal) = err.downcast_ref::<calimero_governance_store::NodeDeviceError>() {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: refusal.to_string(),
        };
    }
    // The typed refusals the governance store and the execute path already raise.
    // `{err:#}` rather than `to_string()`: a caller may have wrapped the refusal
    // (`wrap_err("execution failed")`), and the wrapper alone says nothing the
    // client can act on. The alternate form prints the whole chain.
    if let Some(refusal) = err.downcast_ref::<ExecuteError>() {
        if let Some(status_code) = execute_refusal_status(refusal) {
            return ApiError {
                status_code,
                message: format!("{err:#}"),
            };
        }
    }
    if let Some(refusal) = err.downcast_ref::<NamespaceError>() {
        if let Some(status_code) = namespace_refusal_status(refusal) {
            return ApiError {
                status_code,
                message: format!("{err:#}"),
            };
        }
    }
    if let Some(refusal) = err.downcast_ref::<ApplyError>() {
        if let Some(status_code) = apply_refusal_status(refusal) {
            return ApiError {
                status_code,
                message: format!("{err:#}"),
            };
        }
    }
    if let Some(CapabilitiesError::Unauthorized { .. }) = err.downcast_ref::<CapabilitiesError>() {
        return ApiError {
            status_code: StatusCode::FORBIDDEN,
            message: format!("{err:#}"),
        };
    }
    if let Some(ContextRegistrationError::NotInGroup { .. }) =
        err.downcast_ref::<ContextRegistrationError>()
    {
        return ApiError {
            status_code: StatusCode::NOT_FOUND,
            message: format!("{err:#}"),
        };
    }
    if let Some(refusal) = err.downcast_ref::<MetaError>() {
        return ApiError {
            status_code: match refusal {
                MetaError::GroupNotFoundForHash => StatusCode::NOT_FOUND,
                MetaError::HasRegisteredContexts => StatusCode::CONFLICT,
            },
            message: format!("{err:#}"),
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

#[cfg(test)]
mod static_asset_tests {
    use super::{apply_dashboard_security_headers, is_rewritable_text, rewrite_dashboard_paths};

    #[test]
    fn dashboard_responses_refuse_framing_and_sniffing() {
        let mut headers = axum::http::HeaderMap::new();
        let _previous = headers.insert("x-frame-options", "SAMEORIGIN".parse().unwrap());

        apply_dashboard_security_headers(&mut headers);

        let csp = headers["content-security-policy"].to_str().unwrap();
        assert!(csp.contains("frame-ancestors 'none'"));
        assert!(csp.contains("object-src 'none'"));
        assert!(csp.contains("base-uri 'self'"));
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["x-content-type-options"], "nosniff");
        assert_eq!(headers["referrer-policy"], "no-referrer");
    }

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

    use calimero_governance_store::MembershipError;

    /// The bug this arm exists for. A member who is admin of a *subgroup* but
    /// not of the namespace root is refused by `require_admin` — correctly —
    /// and the caller was told `500 {"error":"Internal server error"}`. A UI
    /// cannot distinguish that from the node falling over, so it cannot say
    /// "you are not allowed" and cannot decide whether to retry.
    #[test]
    fn not_admin_maps_to_403_with_the_reason() {
        let err = MembershipError::NotAdmin {
            group_id: "channel-subgroup".to_owned(),
            identity: "namespace-admin".to_owned(),
        };
        let api = parse_api_error(err.into());
        assert_eq!(api.status_code, StatusCode::FORBIDDEN);
        assert!(
            api.message.contains("is not an admin"),
            "the refusal's own words are what tell a client what to fix; got: {}",
            api.message
        );
    }

    #[test]
    fn not_member_maps_to_403() {
        let err = MembershipError::NotMember {
            group_id: "g".to_owned(),
            identity: "i".to_owned(),
        };
        assert_eq!(
            parse_api_error(err.into()).status_code,
            StatusCode::FORBIDDEN
        );
    }

    /// Moving an attested TEE out of the TEE roles is refused whatever the
    /// caller's authority, like the other TEE role refusals: 403, with the
    /// message saying what to do instead.
    #[test]
    fn a_tee_member_role_lock_maps_to_403_and_says_remove_it() {
        let err = MembershipError::TeeMemberRoleLocked {
            member: "tee".to_owned(),
            current: "ReadOnlyTee".to_owned(),
            requested: "Member".to_owned(),
        };
        let api = parse_api_error(err.into());
        assert_eq!(api.status_code, StatusCode::FORBIDDEN);
        assert!(
            api.message.contains("attested TEE") && api.message.contains("remove it instead"),
            "{}",
            api.message
        );
    }

    /// "You may not" and "not while the group looks like this" are different
    /// answers. Removing the last admin is refused no matter who asks, so it is
    /// a conflict with state, not an authorization failure — a client that
    /// retries after escalating privileges is wasting its time.
    #[test]
    fn last_admin_maps_to_409_not_403() {
        let api = parse_api_error(MembershipError::LastAdmin.into());
        assert_eq!(api.status_code, StatusCode::CONFLICT);
    }

    #[test]
    fn unknown_group_maps_to_404() {
        let err = MembershipError::UnknownGroup("g".to_owned());
        assert_eq!(
            parse_api_error(err.into()).status_code,
            StatusCode::NOT_FOUND
        );
    }

    /// Store corruption is the one family here that really IS this node's
    /// fault, and it must keep answering 500 — and must NOT echo its message,
    /// which names internal rows.
    #[test]
    fn store_corruption_stays_500_and_stays_quiet() {
        let err = MembershipError::MissingMemberValue {
            group_id: "g".to_owned(),
            account: "a".to_owned(),
        };
        let api = parse_api_error(err.into());
        assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(api.message, "Internal server error");
    }

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

    #[test]
    fn a_narrowed_device_maps_to_403_with_message() {
        let err = calimero_context::error::ContextError::DeviceOutOfScope {
            group_id: "test-group".to_owned(),
        };
        let api = parse_api_error(err.into());
        assert_eq!(api.status_code, StatusCode::FORBIDDEN);
        assert!(
            api.message.contains("narrowed its application scope"),
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

    /// The five refusals both pairing handlers raise, and the one the device slot
    /// underneath them raises. Every one of them used to answer
    /// `500 {"error":"Internal server error"}`, so a client could not tell a
    /// mistyped code from a node that had not synced a scope key yet.
    mod pairing {
        use calimero_context::error::ContextError;

        use super::{parse_api_error, StatusCode};

        /// The statement is the caller's bytes, so a caller can fix it.
        #[test]
        fn an_unverifiable_statement_maps_to_400() {
            let api = parse_api_error(
                ContextError::PairingStatementInvalid {
                    device: "d".to_owned(),
                    cause: "pairing statement signature is invalid".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
            assert!(
                api.message.contains("account pair-init"),
                "the refusal should say how to get a good statement; got: {}",
                api.message
            );
        }

        /// And it must still never echo the code the caller failed to supply -
        /// an attacker driving this endpoint would otherwise be handed the one
        /// value it cannot compute.
        #[test]
        fn a_mismatched_confirmation_code_maps_to_400_without_the_expected_code() {
            let api = parse_api_error(
                ContextError::PairingCodeMismatch {
                    device: "d".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
            assert!(
                !api.message.chars().any(|c| c.is_ascii_digit()),
                "a confirmation code is hex, so no digit should reach the caller; got: {}",
                api.message
            );
        }

        /// `409`, not `400`: the request is well-formed and the identical call
        /// works once this node takes part in the namespaces it names.
        #[test]
        fn a_scope_this_node_signs_nowhere_in_maps_to_409() {
            let api = parse_api_error(
                ContextError::PairingNoNamespaceIdentity {
                    namespaces: "[]".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::CONFLICT);
        }

        #[test]
        fn no_current_scope_key_maps_to_409() {
            let api = parse_api_error(
                ContextError::PairingNoScopeKey {
                    namespaces: "[]".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::CONFLICT);
        }

        /// The right request at the wrong node, which no retry and no sync fixes.
        #[test]
        fn a_node_that_does_not_hold_the_account_maps_to_403() {
            let api = parse_api_error(
                ContextError::PairingNotTheAccountHolder {
                    enrolled: "a".to_owned(),
                    account: "b".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(
                api.message.contains("the node that holds the account"),
                "the caller should learn where to run this; got: {}",
                api.message
            );
        }

        /// Raised a crate deeper, in `calimero-governance-store`, and it reaches
        /// the client only because the context handler propagates the report
        /// rather than restating it.
        #[test]
        fn a_device_linked_to_another_account_maps_to_403() {
            let api = parse_api_error(
                calimero_governance_store::NodeDeviceError::LinkedToAnotherAccount {
                    device: "d".to_owned(),
                    account: "a".to_owned(),
                    namespace: "ns".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(
                api.message.contains("revoking the existing device first"),
                "the refusal has to say what to do next; got: {}",
                api.message
            );
        }

        /// A write from a device that was revoked and has no root to fall back to
        /// is a client error naming the revocation, never a 500.
        #[test]
        fn a_revoked_device_with_no_root_maps_to_403_and_names_the_revocation() {
            let api = parse_api_error(
                eyre::Report::from(calimero_governance_store::NodeDeviceError::Revoked {
                    device: "d".to_owned(),
                    account: "a".to_owned(),
                    namespaces: "[ns]".to_owned(),
                })
                .wrap_err("failed to mint this node's account credential"),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(
                api.message.contains("revoked from account a"),
                "{}",
                api.message
            );
        }

        /// A scope replacement that names no application at all. `400`: the
        /// caller has to fix the payload, and `all` is the request they meant.
        #[test]
        fn an_empty_scope_replacement_maps_to_400() {
            let api = parse_api_error(ContextError::ScopeReplacementEmpty.into());
            assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
        }

        /// A scope replacement naming the device that holds the account root.
        /// `403`: the request was understood and can never work, on any node.
        #[test]
        fn rescoping_the_root_holding_device_maps_to_403() {
            let api = parse_api_error(
                ContextError::ScopeReplacementHoldsTheRoot {
                    device: "d".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(
                api.message.contains("holds the account root")
                    && api.message.contains("nothing to replace"),
                "the refusal has to say why there is nothing to do; got: {}",
                api.message
            );
        }

        /// A device whose scope epochs are spent. `409`: the request is understood
        /// and conflicts with a state no retry moves.
        #[test]
        fn a_spent_scope_epoch_maps_to_409() {
            let api = parse_api_error(
                ContextError::ScopeEpochExhausted {
                    device: "d".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::CONFLICT);
            assert!(api.message.contains("last scope epoch"), "{}", api.message);
        }

        /// A relink names a device this node holds no certificate for. `404`,
        /// because the thing being addressed does not exist here - not `403`,
        /// which would say the caller is at the wrong machine.
        #[test]
        fn an_unknown_device_maps_to_404() {
            let api = parse_api_error(
                ContextError::PairingUnknownDevice {
                    device: "d".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::NOT_FOUND);
        }

        /// A revocation naming a device the namespace holds no binding for.
        /// `404`, like the relink above: the thing being addressed is not here.
        #[test]
        fn revoking_an_unknown_device_maps_to_404() {
            let api = parse_api_error(
                ContextError::RevocationUnknownDevice {
                    namespace: "ContextGroupId(a1)".to_owned(),
                    device: "d".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::NOT_FOUND);
        }

        /// A carried device link that does not verify is the caller's to re-sign
        /// (`400`); one this node will never carry - a revoked device, an account
        /// the namespace does not know - is a `403`.
        #[test]
        fn carried_device_link_refusals_map_to_400_and_403() {
            let invalid = parse_api_error(
                ContextError::DeviceLinkInvalid {
                    reason: "scope".to_owned(),
                }
                .into(),
            );
            assert_eq!(invalid.status_code, StatusCode::BAD_REQUEST);
            let refused = parse_api_error(
                ContextError::DeviceLinkRefused {
                    reason: "stranger".to_owned(),
                }
                .into(),
            );
            assert_eq!(refused.status_code, StatusCode::FORBIDDEN);
            assert!(refused.message.contains("stranger"), "{}", refused.message);
        }

        /// A revocation naming the device this node runs as. `403`: the request
        /// is understood and can never succeed from this node, and the message
        /// has to say where it can be done instead.
        #[test]
        fn revoking_this_nodes_own_device_maps_to_403() {
            let api = parse_api_error(
                ContextError::RevocationOfOwnDevice {
                    device: "d".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(
                api.message.contains("another device of the account"),
                "the refusal has to say where the revocation can be run; got: {}",
                api.message
            );
        }

        /// Leaving a namespace root through the group leave. `400`, and the
        /// message names the route that does it.
        #[test]
        fn leaving_a_namespace_root_as_a_group_maps_to_400() {
            let api = parse_api_error(
                ContextError::LeaveGroupIsNamespace {
                    group_id: "ContextGroupId(a1)".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
            assert!(
                api.message
                    .contains("/admin-api/namespaces/{namespace_id}/leave"),
                "the refusal has to name the namespace leave; got: {}",
                api.message
            );
        }

        /// Leaving a group this node reaches only through its parent. `409`,
        /// and the message says where the leave belongs.
        #[test]
        fn leaving_a_group_held_only_through_its_parent_maps_to_409() {
            let api = parse_api_error(
                ContextError::LeaveGroupNotDirectMember {
                    group_id: "ContextGroupId(a1)".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::CONFLICT);
            assert!(
                api.message.contains("leave the parent group"),
                "the refusal has to say where the leave belongs; got: {}",
                api.message
            );
        }

        /// Retrying a group that was never upgraded. `404`: there is no upgrade
        /// to retry.
        #[test]
        fn retrying_a_group_never_upgraded_maps_to_404() {
            let api = parse_api_error(
                ContextError::UpgradeNotFound {
                    group_id: "ContextGroupId(a1)".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::NOT_FOUND);
        }

        /// Every other upgrade precondition the group's state fails. `409`: the
        /// request is understood and conflicts with where the group is.
        #[test]
        fn upgrade_preconditions_map_to_409() {
            let group_id = || "ContextGroupId(a1)".to_owned();
            for err in [
                ContextError::UpgradeInProgress {
                    group_id: group_id(),
                },
                ContextError::UpgradeAlreadyTargeting {
                    group_id: group_id(),
                },
                ContextError::UpgradeNoContexts {
                    group_id: group_id(),
                },
                ContextError::UpgradeNotRetryable {
                    group_id: group_id(),
                    reason: "is already completed",
                },
            ] {
                let message = err.to_string();
                let api = parse_api_error(err.into());
                assert_eq!(api.status_code, StatusCode::CONFLICT, "{message}");
                assert_eq!(api.message, message);
            }
        }

        /// The upgrade gate refusing a target. `409`, with the gate's own message
        /// unchanged: it is what says which target to pick instead.
        #[test]
        fn an_upgrade_the_gate_refuses_maps_to_409_with_its_reason() {
            let reason = "identity downgrade forbidden: field 'posts' AuthoredMap -> \
                          UnorderedMap strips authorship/writer-ACL network-wide";
            let api = parse_api_error(
                ContextError::UpgradeRefused {
                    reason: reason.to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::CONFLICT);
            assert_eq!(api.message, reason);
        }

        /// A TEE policy that cannot be stored as sent. `400`, with the check's
        /// own message: it names the field and where its value comes from.
        #[test]
        fn an_unusable_tee_policy_maps_to_400_with_its_reason() {
            let reason = "allowed_mrtd must name at least one measurement";
            let api = parse_api_error(
                ContextError::TeePolicyInvalid {
                    reason: reason.to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
            assert_eq!(api.message, reason);
        }

        /// An ownership-proof request the node will not sign as given is a `400`
        /// naming the field; one from a node that is not a direct admin is a
        /// `403`, since only being granted the role helps.
        #[test]
        fn ownership_proof_refusals_map_to_400_and_403() {
            let reason = "ownership-proof `nonce` must be at least 8 bytes";
            let api = parse_api_error(
                ContextError::OwnershipProofInvalid {
                    reason: reason.to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::BAD_REQUEST);
            assert_eq!(api.message, reason);

            let api = parse_api_error(
                ContextError::NotAGroupAdmin {
                    group_id: "ContextGroupId(a1)".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(api.message.contains("direct admin"), "{}", api.message);
        }

        /// Group-creation refusals: a taken id is a `409`, a nested subgroup
        /// without namespace admin a `403`, an unusable `bytecode_id` a `400`,
        /// and one naming a blob this node does not hold a `404`.
        #[test]
        fn group_creation_refusals_map_by_what_the_caller_should_do() {
            for (err, status) in [
                (
                    ContextError::GroupAlreadyExists {
                        group_id: "ContextGroupId(a1)".to_owned(),
                    },
                    StatusCode::CONFLICT,
                ),
                (
                    ContextError::SubgroupCreationNeedsNamespaceAdmin {
                        parent_id: "ContextGroupId(a1)".to_owned(),
                    },
                    StatusCode::FORBIDDEN,
                ),
                (
                    ContextError::BytecodeIdInvalid {
                        reason: "bytecode_id must not be zero".to_owned(),
                    },
                    StatusCode::BAD_REQUEST,
                ),
                (
                    ContextError::BytecodeNotInstalled {
                        blob_id: "b1".to_owned(),
                    },
                    StatusCode::NOT_FOUND,
                ),
            ] {
                let message = err.to_string();
                let api = parse_api_error(err.into());
                assert_eq!(api.status_code, status, "{message}");
                assert_eq!(api.message, message);
            }
        }

        /// A caller this node cannot act as is a `403`, and the message it reads
        /// names neither the caller nor the context.
        #[test]
        fn an_unpermitted_caller_maps_to_403_without_naming_anyone() {
            let api = parse_api_error(ContextError::CallerNotPermitted.into());
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert_eq!(
                api.message,
                "unauthorized: caller is not a permitted identity for this context"
            );
        }

        /// A device name this node cannot mint, a seed that collides with a
        /// held context, and a resync the context does not admit: each a `409`
        /// with its message unchanged.
        #[test]
        fn node_state_refusals_map_to_409() {
            for err in [
                ContextError::DeviceLabelUnavailable {
                    reason: "this node holds no usable device, so it can name none".to_owned(),
                },
                ContextError::ContextSeedCollision,
                ContextError::ResyncRefused {
                    context_id: "c1".to_owned(),
                    reason: "not in a group; resync recovery is group-only".to_owned(),
                },
            ] {
                let message = err.to_string();
                let api = parse_api_error(err.into());
                assert_eq!(api.status_code, StatusCode::CONFLICT, "{message}");
                assert_eq!(api.message, message);
            }
        }

        /// An expired invitation is a `409` like a consumed one, a malformed one
        /// a `400`, and a join whose key has not arrived yet a `503`: the three
        /// ask a client for a fresh invitation, a fixed one, and a retry.
        #[test]
        fn join_refusals_map_by_what_the_caller_should_do() {
            let group_id = || "ContextGroupId(a1)".to_owned();
            for (err, status) in [
                (
                    ContextError::InvitationExpired {
                        group_id: group_id(),
                        expired_at: 1,
                    },
                    StatusCode::CONFLICT,
                ),
                (
                    ContextError::InvitationInvalid {
                        group_id: group_id(),
                        reason: "it carries no application_id",
                    },
                    StatusCode::BAD_REQUEST,
                ),
                (
                    ContextError::JoinKeyDeliveryTimedOut {
                        group_id: group_id(),
                        waited_secs: 5,
                    },
                    StatusCode::SERVICE_UNAVAILABLE,
                ),
            ] {
                let message = err.to_string();
                let api = parse_api_error(err.into());
                assert_eq!(api.status_code, status, "{message}");
                assert_eq!(api.message, message);
            }
        }

        /// And a revoked one to `403`, permanently: re-enrolling the machine mints
        /// a FRESH device id, so no sequence of calls makes this id work again -
        /// which the message has to say rather than imply an un-revoke.
        #[test]
        fn a_revoked_device_maps_to_403_and_says_re_enrolling_mints_a_new_id() {
            let api = parse_api_error(
                ContextError::PairingDeviceRevoked {
                    device: "d".to_owned(),
                    namespaces: "[]".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(
                api.message.contains("mints a new device id"),
                "the caller must learn that the id is spent for good; got: {}",
                api.message
            );
        }
    }

    /// "The thing you named is not here" must be a `404`.
    ///
    /// These were bare `bail!("group '…' not found")`, which `parse_api_error`
    /// had nothing to match, so they came back as
    /// `500 {"error":"Internal server error"}`. Two things went wrong with
    /// that during the fleet-HA incident: a control-plane wrapper around
    /// namespace-leave read the 500 as "likely already left / not a member"
    /// and carried on past a real failure, and a dashboard polling an absent
    /// group every ~1.4s turned a routine miss into a permanent ERROR stream
    /// in the operator's central log store.
    mod absent_resources_are_404_not_500 {
        use calimero_context::error::ContextError;

        use super::*;

        #[test]
        fn a_missing_group_is_404_and_keeps_its_message() {
            let api = parse_api_error(
                ContextError::GroupNotFound {
                    group_id: "ContextGroupId(f72d)".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::NOT_FOUND);
            assert!(
                api.message.contains("f72d"),
                "the id the caller supplied is what tells them what was missing; got: {}",
                api.message
            );
        }

        #[test]
        fn missing_namespace_application_and_context_are_404_too() {
            for err in [
                ContextError::NamespaceNotFound {
                    namespace_id: "n".to_owned(),
                },
                ContextError::ApplicationNotFound {
                    application_id: "a".to_owned(),
                },
                ContextError::ContextNotFound {
                    context_id: "c".to_owned(),
                },
            ] {
                let rendered = err.to_string();
                assert_eq!(
                    parse_api_error(err.into()).status_code,
                    StatusCode::NOT_FOUND,
                    "{rendered}"
                );
            }
        }

        /// The queried identity not being a member is the same category as the
        /// group being absent: the caller asked about something that is not
        /// there.
        #[test]
        fn a_non_member_identity_is_404() {
            let api = parse_api_error(
                MembershipError::MemberNotFound {
                    group_id: "g".to_owned(),
                    member: "m".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::NOT_FOUND);
        }

        /// Not a member is `403`, not `404`: the group exists, the node just
        /// has no standing in it, and telling a caller "not found" would send
        /// them looking for a group that is right there.
        #[test]
        fn not_a_member_stays_403_for_groups_and_namespaces() {
            for err in [
                ContextError::NotAGroupMember {
                    group_id: "g".to_owned(),
                },
                ContextError::NotANamespaceMember {
                    namespace_id: "n".to_owned(),
                },
            ] {
                assert_eq!(
                    parse_api_error(err.into()).status_code,
                    StatusCode::FORBIDDEN
                );
            }
        }

        /// Why typing was needed at all, pinned: the untyped form these sites
        /// used still falls through to the generic 500 with its message
        /// scrubbed. This is what every one of them returned before.
        #[test]
        fn an_untyped_not_found_still_falls_through_to_500() {
            let api = parse_api_error(eyre::eyre!("group 'ContextGroupId(f72d)' not found"));
            assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(api.message, "Internal server error");
        }

        /// A caller-supplied identity is not this node, and the message must
        /// not claim it is.
        ///
        /// `create_context` takes `identity_secret` straight from the request
        /// body, so the identity it checks is routinely somebody else's.
        /// Reusing `NotAGroupMember` there answered "node is not a member of
        /// group X" about a principal that was never the node.
        #[test]
        fn a_caller_supplied_identity_is_403_and_names_the_identity_not_the_node() {
            let api = parse_api_error(
                ContextError::IdentityNotAGroupMember {
                    group_id: "g".to_owned(),
                    identity: "ed25519:caller".to_owned(),
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(
                api.message.contains("ed25519:caller"),
                "the refusal must name the identity that was checked; got: {}",
                api.message
            );
            assert!(
                !api.message.contains("node is not a member"),
                "and must not claim the NODE is the one without standing; got: {}",
                api.message
            );
        }

        /// The log-level split rides on this, and an ERROR per poll on a fleet
        /// node is shipped to the operator's log store.
        #[test]
        fn client_fault_tracks_the_status_class() {
            let not_found = parse_api_error(
                ContextError::GroupNotFound {
                    group_id: "g".to_owned(),
                }
                .into(),
            );
            assert!(not_found.is_client_fault());

            let server_fault = parse_api_error(eyre::eyre!("something internal broke"));
            assert!(!server_fault.is_client_fault());
        }
    }

    /// The typed refusals `parse_api_error` did not map before: the execute
    /// path's `ExecuteError`, and the governance store's namespace, apply,
    /// capability, registration and meta errors. Every one of them used to be
    /// the generic `500`.
    mod typed_refusals {
        use calimero_context::error::ContextError;
        use calimero_context_client::messages::{ExecuteError, InternalErrorKind};
        use calimero_governance_store::{
            ApplyError, CapabilitiesError, ContextRegistrationError, GroupCreatedRejection,
            MemberJoinedOpenRejection, MembershipError, MetaError, NamespaceCreatedRejection,
            NamespaceError,
        };
        use calimero_primitives::context::ContextId;

        use super::{parse_api_error, StatusCode};

        fn status(err: eyre::Report) -> StatusCode {
            parse_api_error(err).status_code
        }

        #[test]
        fn execute_errors_map_by_what_the_caller_can_do_next() {
            assert_eq!(
                status(ExecuteError::ContextNotFound.into()),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                status(ExecuteError::Uninitialized.into()),
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert_eq!(
                status(
                    ExecuteError::GroupKeyPending {
                        context_id: ContextId::from([7; 32]),
                    }
                    .into()
                ),
                StatusCode::SERVICE_UNAVAILABLE
            );
            assert_eq!(
                status(
                    ExecuteError::XCallNotPermitted {
                        context_id: ContextId::from([7; 32]),
                    }
                    .into()
                ),
                StatusCode::FORBIDDEN
            );
        }

        /// An internal execution failure stays the generic 500, message and all.
        #[test]
        fn an_internal_execute_error_stays_a_quiet_500() {
            let api = parse_api_error(
                ExecuteError::InternalError {
                    kind: InternalErrorKind::Runtime,
                }
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(api.message, "Internal server error");
        }

        /// The intent route wraps execution errors (`wrap_err("execution
        /// failed")`). The wrapper must neither hide the status nor the reason.
        #[test]
        fn a_wrapped_execute_error_keeps_its_status_and_its_reason() {
            let api = parse_api_error(
                eyre::Report::new(ExecuteError::ContextNotFound).wrap_err("execution failed"),
            );
            assert_eq!(api.status_code, StatusCode::NOT_FOUND);
            assert!(
                api.message.contains("context not found"),
                "the wrapper alone says nothing actionable; got: {}",
                api.message
            );
        }

        /// A delegated write the execute path refused over a role reaches the
        /// `/intents` caller as a 403 naming the role — never as the `200` it
        /// used to be (writes silently discarded) or the opaque `500`.
        #[test]
        fn a_delegated_write_refused_over_a_role_is_a_403_with_its_reason() {
            use calimero_context_client::messages::DelegatedWriteRefusal;
            for (reason, says) in [
                (
                    DelegatedWriteRefusal::ExecutorIsTeeReplica,
                    "this node is a TEE replica (ReadOnlyTee) and does not relay writes",
                ),
                (
                    DelegatedWriteRefusal::ExecutorIsReadOnly,
                    "this node's role in this context is read-only (ReadOnly), so it does not relay writes",
                ),
                (
                    DelegatedWriteRefusal::AuthorIsReadOnly,
                    "the author's role in this context is read-only",
                ),
            ] {
                let api = parse_api_error(
                    eyre::Report::new(ExecuteError::DelegatedWriteRefused {
                        context_id: ContextId::from([7; 32]),
                        reason,
                    })
                    .wrap_err("execution failed"),
                );
                assert_eq!(api.status_code, StatusCode::FORBIDDEN);
                assert!(api.message.contains(says), "got: {}", api.message);
            }
        }

        #[test]
        fn reparent_refusals_are_client_errors_by_kind() {
            assert_eq!(
                status(NamespaceError::RootHasNoParent("root".to_owned()).into()),
                StatusCode::BAD_REQUEST
            );
            assert_eq!(
                status(NamespaceError::ReparentTargetMissing("p".to_owned()).into()),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                status(
                    NamespaceError::ReparentCycle {
                        new_parent: "p".to_owned(),
                        child: "c".to_owned(),
                    }
                    .into()
                ),
                StatusCode::CONFLICT
            );
        }

        /// A tree too deep to walk is the store's problem, not the request's.
        #[test]
        fn an_unwalkable_tree_stays_a_quiet_500() {
            let api = parse_api_error(NamespaceError::DepthExceeded.into());
            assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(api.message, "Internal server error");
        }

        #[test]
        fn apply_refusals_map_through_their_wrappers() {
            assert_eq!(
                status(
                    ApplyError::GroupCreatedRejected(GroupCreatedRejection::Unauthorized {
                        signer: "s".to_owned(),
                        namespace: "n".to_owned(),
                    })
                    .into()
                ),
                StatusCode::FORBIDDEN
            );
            assert_eq!(
                status(
                    ApplyError::MemberJoinedOpenRejected(
                        MemberJoinedOpenRejection::ReentryBlocked {
                            member: "m".to_owned(),
                            gid: "g".to_owned(),
                        }
                    )
                    .into()
                ),
                StatusCode::CONFLICT
            );
            assert_eq!(
                status(
                    ApplyError::AuthorityUndecidable {
                        group_id: "g".to_owned(),
                        signer: "s".to_owned(),
                    }
                    .into()
                ),
                StatusCode::SERVICE_UNAVAILABLE
            );
        }

        /// A genesis this node signed itself and got wrong is its own fault.
        #[test]
        fn a_rejected_genesis_stays_a_quiet_500() {
            let api = parse_api_error(
                ApplyError::NamespaceCreatedRejected(NamespaceCreatedRejection::NotGenesis {
                    parent_count: 1,
                })
                .into(),
            );
            assert_eq!(api.status_code, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(api.message, "Internal server error");
        }

        /// The shape `delete_group` now produces: the capability refusal wrapped
        /// with what was being attempted.
        #[test]
        fn a_wrapped_capability_refusal_is_403_and_says_both_what_and_why() {
            let api = parse_api_error(
                eyre::Report::new(CapabilitiesError::Unauthorized {
                    group_id: "g".to_owned(),
                    operation: "delete subgroup".to_owned(),
                })
                .wrap_err("deleting subgroup 'g' needs its owner or CAN_DELETE_SUBGROUP"),
            );
            assert_eq!(api.status_code, StatusCode::FORBIDDEN);
            assert!(api.message.contains("deleting subgroup"), "{}", api.message);
            assert!(api.message.contains("lacks permission"), "{}", api.message);
        }

        /// The shape the join relay now produces: a membership refusal wrapped
        /// with context. It used to be formatted into an untyped string.
        #[test]
        fn a_wrapped_membership_refusal_keeps_its_status() {
            let api = parse_api_error(
                eyre::Report::new(MembershipError::LastAdmin)
                    .wrap_err("could not sign and apply this join locally"),
            );
            assert_eq!(api.status_code, StatusCode::CONFLICT);
        }

        #[test]
        fn an_unregistered_context_is_404_and_a_group_with_contexts_is_409() {
            assert_eq!(
                status(
                    ContextRegistrationError::NotInGroup {
                        group_id: "g".to_owned(),
                        context_id: "c".to_owned(),
                    }
                    .into()
                ),
                StatusCode::NOT_FOUND
            );
            assert_eq!(
                status(MetaError::HasRegisteredContexts.into()),
                StatusCode::CONFLICT
            );
        }

        /// Both are retryable, but they are not the same wait: nobody answered,
        /// versus someone answered who cannot admit you.
        #[test]
        fn an_unendorsed_join_is_retryable_and_says_which_kind() {
            let unreachable = parse_api_error(
                ContextError::JoinNotEndorsed {
                    reached_a_peer: false,
                }
                .into(),
            );
            let wrong_peer = parse_api_error(
                ContextError::JoinNotEndorsed {
                    reached_a_peer: true,
                }
                .into(),
            );

            assert_eq!(unreachable.status_code, StatusCode::GATEWAY_TIMEOUT);
            assert_eq!(wrong_peer.status_code, StatusCode::SERVICE_UNAVAILABLE);
            assert!(unreachable.message.contains("could not reach any member"));
            assert!(wrong_peer.message.contains("not an admitter"));
        }
    }
}

#[cfg(test)]
mod admin_config_compat_tests {
    use super::AdminConfig;

    /// A node upgraded in place still reads the key its `config.toml` holds.
    ///
    /// `merod init` wrote `public_intents` for every node created before this
    /// rename, and nothing rewrites an existing config on upgrade. Without the
    /// alias the field would fall to `#[serde(default)]` and the relay would
    /// come up with the delegated surface SHUT — not an error, just quietly
    /// off, which is the failure mode nobody notices until a client reports it.
    #[test]
    fn the_old_config_key_still_sets_the_new_field() {
        let old: AdminConfig =
            serde_json::from_str(r#"{"enabled":true,"public_intents":true}"#).unwrap();
        assert!(
            old.delegated_access,
            "an existing config.toml must keep opening the surface it opened yesterday"
        );

        let new: AdminConfig =
            serde_json::from_str(r#"{"enabled":true,"delegated_access":true}"#).unwrap();
        assert!(new.delegated_access);

        let absent: AdminConfig = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert!(
            !absent.delegated_access,
            "a config that mentions neither must stay shut"
        );
    }

    /// What this node WRITES is the new key, and that is the half the alias
    /// cannot save.
    ///
    /// An older merod does not know `delegated_access`, so it reads a config
    /// written here as "unset" and comes up shut. The compat paths therefore run
    /// in opposite directions. A new binary reading an old key is covered
    /// above; an old binary reading a new key is not, and cannot be. That is
    /// why the fleet image keeps writing the old key until its merod is past
    /// this release — a rollback would otherwise present as "the flag stopped
    /// working" rather than as a parse error anyone could act on.
    #[test]
    fn a_written_config_uses_the_new_key_only() {
        let json = serde_json::to_string(&AdminConfig::new(true, true)).unwrap();
        assert!(json.contains("delegated_access"), "{json}");
        assert!(
            !json.contains("public_intents"),
            "serializing the old name back out would make the rename invisible \
             and never end: {json}"
        );
    }
}
