use std::sync::LazyLock;

use axum::body::Body;
use axum::http::Request;
use regex::Regex;

use super::types::{
    AddBlobPermission, AdminPermission, AliasPermission, AliasType, ApplicationPermission,
    BlobPermission, CapabilityPermission, ContextApplicationPermission, ContextPermission,
    GroupPermission, HttpMethod, KeyPermission, NamespacePermission, PackagePermission, Permission,
    ResourceScope, UserScope,
};

/// Pre-compiled regex patterns for performance
static APPLICATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/applications/([^/]+)$").unwrap());

static CONTEXT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/contexts/([^/]+)$").unwrap());

static CONTEXT_CAPABILITIES_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/contexts/([^/]+)/capabilities/(grant|revoke)$").unwrap()
});

static CONTEXT_APPLICATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/contexts/([^/]+)/application$").unwrap());

static CONTEXTS_FOR_APPLICATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/contexts/for-application/([^/]+)$").unwrap());

static CONTEXTS_WITH_EXECUTORS_FOR_APPLICATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/contexts/with-executors/for-application/([^/]+)$").unwrap()
});

static BLOB_REGEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^/blobs/([^/]+)$").unwrap());

static ADMIN_KEY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin/keys/([^/]+)$").unwrap());

static KEY_PERMISSIONS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin/keys/([^/]+)/permissions$").unwrap());

static CLIENT_MANAGEMENT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin/keys/([^/]+)/clients/([^/]+)$").unwrap());

static PACKAGE_VERSIONS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/packages/([^/]+)/versions$").unwrap());

static PACKAGE_LATEST_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/packages/([^/]+)/latest$").unwrap());

static NAMESPACES_FOR_APPLICATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/for-application/([^/]+)$").unwrap());

static NAMESPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)$").unwrap());

static NAMESPACE_READ_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)/(identity|groups)$").unwrap());

static NAMESPACE_MEMBERSHIP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)/(invite|join|leave)$").unwrap());

static GROUP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/groups/([^/]+)(?:/.*)?$").unwrap());

static CONTEXT_READ_SUBRESOURCE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/contexts/([^/]+)/(identities|identities-owned|storage|group)$")
        .unwrap()
});

static CONTEXT_MEMBERSHIP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/contexts/([^/]+)/(join|leave|resync)$").unwrap());

static CONTEXT_SYNC_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/contexts/sync/([^/]+)$").unwrap());

static ADMIN_BLOB_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/blobs/([^/]+)$").unwrap());

static ALIAS_MUTATION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/alias/(create|lookup|delete)/(context|application|identity)(?:/.*)?$")
        .unwrap()
});

static ALIAS_LIST_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/alias/list/(context|application|identity)(?:/.*)?$").unwrap()
});

static APPLICATION_VERSIONS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/applications/([^/]+)/versions$").unwrap());

/// Map an alias path segment to its [`AliasType`]. The regexes only capture
/// `context|application|identity`, so every capture maps.
fn alias_type_from_segment(segment: &str) -> AliasType {
    match segment {
        "context" => AliasType::Context,
        "application" => AliasType::Application,
        _ => AliasType::Identity,
    }
}

/// Permission validator for checking request permissions
#[derive(Debug, Default)]
pub struct PermissionValidator;

fn get_permissions_for_path_with_params(path: &str, method: &HttpMethod) -> Vec<Permission> {
    // Handle parameterized routes with pre-compiled regex patterns
    if let Some(captures) = APPLICATION_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            return match method {
                HttpMethod::GET => vec![Permission::Application(ApplicationPermission::List(
                    ResourceScope::Specific(vec![app_id.as_str().to_string()]),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = CONTEXT_REGEX.captures(path) {
        if let Some(ctx_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                HttpMethod::DELETE => vec![Permission::Context(ContextPermission::Delete(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = CONTEXT_CAPABILITIES_REGEX.captures(path) {
        if let (Some(ctx_id), Some(action)) = (captures.get(1), captures.get(2)) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match (method, action.as_str()) {
                (HttpMethod::POST, "grant") => vec![Permission::Context(
                    ContextPermission::Capabilities(CapabilityPermission::Grant(scope)),
                )],
                (HttpMethod::POST, "revoke") => vec![Permission::Context(
                    ContextPermission::Capabilities(CapabilityPermission::Revoke(scope)),
                )],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = CONTEXT_APPLICATION_REGEX.captures(path) {
        if let Some(ctx_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Application(
                    ContextApplicationPermission::Update(scope),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = CONTEXTS_FOR_APPLICATION_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![app_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = CONTEXTS_WITH_EXECUTORS_FOR_APPLICATION_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![app_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = BLOB_REGEX.captures(path) {
        if let Some(blob_id) = captures.get(1) {
            return match method {
                HttpMethod::DELETE => vec![Permission::Blob(BlobPermission::Remove(
                    ResourceScope::Specific(vec![blob_id.as_str().to_string()]),
                ))],
                _ => vec![],
            };
        }
    }

    // Admin key management endpoints
    if let Some(_captures) = ADMIN_KEY_REGEX.captures(path) {
        return match method {
            HttpMethod::DELETE => vec![Permission::Keys(KeyPermission::Delete)],
            _ => vec![],
        };
    }

    // Key permissions management: /admin/keys/:key_id/permissions
    if let Some(captures) = KEY_PERMISSIONS_REGEX.captures(path) {
        if let Some(key_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![key_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Keys(KeyPermission::GetPermissions(scope))],
                // Updating a key's permissions is privilege management: the
                // handler (`update_key_permissions_handler`) applies whatever
                // permissions the body asks for, including `admin`, without
                // checking that the caller already holds them. Gating the
                // endpoint on a scoped `Keys(UpdatePermissions)` would let a
                // non-admin key that merely holds that scope escalate itself (or
                // any key) to `admin`. Require full `admin` to mutate
                // permissions so escalation is impossible.
                HttpMethod::PUT => vec![Permission::Admin(AdminPermission)],
                _ => vec![],
            };
        }
    }

    // Client management: /admin/keys/:key_id/clients/:client_id
    if let Some(_captures) = CLIENT_MANAGEMENT_REGEX.captures(path) {
        return match method {
            HttpMethod::DELETE => vec![Permission::Keys(KeyPermission::DeleteClient)],
            _ => vec![],
        };
    }

    // Package management endpoints
    if let Some(captures) = PACKAGE_VERSIONS_REGEX.captures(path) {
        if let Some(package) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![package.as_str().to_string()]);
            return match method {
                HttpMethod::GET => {
                    vec![Permission::Package(PackagePermission::ListVersions(scope))]
                }
                _ => vec![],
            };
        }
    }

    if let Some(captures) = PACKAGE_LATEST_REGEX.captures(path) {
        if let Some(package) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![package.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Package(PackagePermission::GetLatestVersion(
                    scope,
                ))],
                _ => vec![],
            };
        }
    }

    // Namespace endpoints (checked before the single-segment namespace regex so
    // `/namespaces/for-application/:app_id` scopes to the application id)
    if let Some(captures) = NAMESPACES_FOR_APPLICATION_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![app_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Namespace(NamespacePermission::List(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Namespace(NamespacePermission::List(scope))],
                HttpMethod::DELETE => {
                    vec![Permission::Namespace(NamespacePermission::Manage(scope))]
                }
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_READ_REGEX.captures(path) {
        if let (Some(ns_id), Some(resource)) = (captures.get(1), captures.get(2)) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match (method, resource.as_str()) {
                (HttpMethod::GET, _) => {
                    vec![Permission::Namespace(NamespacePermission::List(scope))]
                }
                // POST /namespaces/:id/groups creates a group in the namespace
                (HttpMethod::POST, "groups") => {
                    vec![Permission::Group(GroupPermission::Create(
                        ResourceScope::Global,
                    ))]
                }
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_MEMBERSHIP_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                HttpMethod::POST => {
                    vec![Permission::Namespace(NamespacePermission::Manage(scope))]
                }
                _ => vec![],
            };
        }
    }

    // Group governance endpoints: catch-all for `/admin-api/groups/:group_id`
    // and everything nested below it (members, metadata, settings, upgrade,
    // migration, signing keys, ownership proofs, sync, …). Reads require
    // `group:list[<id>]`, mutations `group:manage[<id>]`.
    if let Some(captures) = GROUP_REGEX.captures(path) {
        if let Some(group_id) = captures.get(1) {
            // `/admin-api/groups/join` is an exact route (join by invitation
            // payload), already mapped in `get_permissions_for_exact_paths`;
            // it must not be treated as a group id here.
            if group_id.as_str() == "join" {
                return vec![];
            }
            let scope = ResourceScope::Specific(vec![group_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Group(GroupPermission::List(scope))],
                HttpMethod::POST | HttpMethod::PUT | HttpMethod::PATCH | HttpMethod::DELETE => {
                    vec![Permission::Group(GroupPermission::Manage(scope))]
                }
                HttpMethod::Any => vec![],
            };
        }
    }

    // Context sub-resources: read-only views of a context a member needs
    if let Some(captures) = CONTEXT_READ_SUBRESOURCE_REGEX.captures(path) {
        if let Some(ctx_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                _ => vec![],
            };
        }
    }

    // Context membership operations (join/leave/resync): member-level actions
    if let Some(captures) = CONTEXT_MEMBERSHIP_REGEX.captures(path) {
        if let Some(ctx_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Execute(
                    scope,
                    UserScope::Any,
                    None,
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = CONTEXT_SYNC_REGEX.captures(path) {
        if let Some(ctx_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Execute(
                    scope,
                    UserScope::Any,
                    None,
                ))],
                _ => vec![],
            };
        }
    }

    // Admin-api blob endpoints (nested under /admin-api, unlike the legacy
    // top-level /blobs routes above)
    if let Some(captures) = ADMIN_BLOB_REGEX.captures(path) {
        if let Some(blob_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![blob_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Blob(BlobPermission::Get(scope))],
                HttpMethod::DELETE => vec![Permission::Blob(BlobPermission::Remove(scope))],
                _ => vec![],
            };
        }
    }

    // Alias endpoints: /admin-api/alias/{create,lookup,delete}/{type}[/…]
    if let Some(captures) = ALIAS_MUTATION_REGEX.captures(path) {
        if let (Some(action), Some(kind)) = (captures.get(1), captures.get(2)) {
            let alias_type = alias_type_from_segment(kind.as_str());
            return match (method, action.as_str()) {
                (HttpMethod::POST, "create") => {
                    vec![Permission::Context(ContextPermission::Alias(
                        AliasPermission::Create(alias_type, ResourceScope::Global),
                    ))]
                }
                (HttpMethod::POST, "lookup") => {
                    vec![Permission::Context(ContextPermission::Alias(
                        AliasPermission::Lookup(alias_type, ResourceScope::Global),
                    ))]
                }
                (HttpMethod::POST, "delete") => {
                    vec![Permission::Context(ContextPermission::Alias(
                        AliasPermission::Delete(alias_type, ResourceScope::Global),
                    ))]
                }
                _ => vec![],
            };
        }
    }

    if let Some(captures) = ALIAS_LIST_REGEX.captures(path) {
        if let Some(kind) = captures.get(1) {
            let alias_type = alias_type_from_segment(kind.as_str());
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::Alias(
                    AliasPermission::List(alias_type, ResourceScope::Global),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = APPLICATION_VERSIONS_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![app_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => {
                    vec![Permission::Application(ApplicationPermission::List(scope))]
                }
                _ => vec![],
            };
        }
    }

    vec![]
}

impl PermissionValidator {
    pub fn new() -> Self {
        Self
    }

    /// Get permissions for exact path matches (non-parameterized routes)
    fn get_permissions_for_exact_paths(&self, path: &str, method: &HttpMethod) -> Vec<Permission> {
        match (path, method) {
            // JSON-RPC endpoints
            ("/jsonrpc", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Execute(ResourceScope::Global, UserScope::Any, None),
            )],

            // Admin API - Applications
            ("/admin-api/applications", HttpMethod::GET) => vec![Permission::Application(
                ApplicationPermission::List(ResourceScope::Global),
            )],
            ("/admin-api/install-application", HttpMethod::POST) => vec![Permission::Application(
                ApplicationPermission::Install(ResourceScope::Global),
            )],
            ("/admin-api/uninstall-application", HttpMethod::POST) => {
                vec![Permission::Application(ApplicationPermission::Uninstall(
                    ResourceScope::Global,
                ))]
            }

            // Admin API - Package Management
            ("/admin-api/packages", HttpMethod::GET) => vec![Permission::Package(
                PackagePermission::ListPackages(ResourceScope::Global),
            )],

            // Admin API - Contexts
            ("/admin-api/contexts", HttpMethod::GET) => vec![Permission::Context(
                ContextPermission::List(ResourceScope::Global),
            )],
            ("/admin-api/contexts", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Create(ResourceScope::Global),
            )],
            ("/admin-api/contexts/sync", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Execute(ResourceScope::Global, UserScope::Any, None),
            )],
            // Generating a fresh context identity keypair is a member-level
            // operation: it grants no access to any existing resource.
            ("/admin-api/identity/context", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Execute(ResourceScope::Global, UserScope::Any, None),
            )],

            // Admin API - Namespaces
            ("/admin-api/namespaces", HttpMethod::GET) => vec![Permission::Namespace(
                NamespacePermission::List(ResourceScope::Global),
            )],
            ("/admin-api/namespaces", HttpMethod::POST) => vec![Permission::Namespace(
                NamespacePermission::Create(ResourceScope::Global),
            )],

            // Admin API - Groups
            ("/admin-api/groups", HttpMethod::POST) => vec![Permission::Group(
                GroupPermission::Create(ResourceScope::Global),
            )],
            ("/admin-api/groups/join", HttpMethod::POST) => vec![Permission::Group(
                GroupPermission::Manage(ResourceScope::Global),
            )],

            // Admin API - Blobs
            ("/admin-api/blobs", HttpMethod::PUT) => vec![Permission::Blob(BlobPermission::Add(
                AddBlobPermission::Stream,
            ))],
            ("/admin-api/blobs", HttpMethod::GET) => vec![Permission::Blob(BlobPermission::List(
                ResourceScope::Global,
            ))],

            // Admin auth endpoints
            ("/admin/keys", HttpMethod::GET) => vec![Permission::Keys(KeyPermission::List)],
            // Creating a key here mints a ROOT key with `["admin"]` permissions
            // (see `create_key_handler` -> `provider.create_root_key`). Gating
            // it on the strictly-weaker `keys:create` let any key holding only
            // `keys:create` mint a full admin root key — privilege escalation.
            // Require `admin`, mirroring the `PUT /admin/keys/:id/permissions`
            // decision.
            ("/admin/keys", HttpMethod::POST) => vec![Permission::Admin(AdminPermission)],
            ("/admin/revoke", HttpMethod::POST) => vec![Permission::Keys(KeyPermission::Delete)],
            ("/admin/keys/clients", HttpMethod::GET) => {
                vec![Permission::Keys(KeyPermission::ListClients)]
            }

            // Blob endpoints
            ("/blobs/stream", HttpMethod::POST) => vec![Permission::Blob(BlobPermission::Add(
                AddBlobPermission::Stream,
            ))],
            ("/blobs/file", HttpMethod::POST) => vec![Permission::Blob(BlobPermission::Add(
                AddBlobPermission::File,
            ))],
            ("/blobs/url", HttpMethod::POST) => vec![Permission::Blob(BlobPermission::Add(
                AddBlobPermission::Url,
            ))],

            _ => vec![],
        }
    }

    /// Determine required permissions for a given request
    pub fn determine_required_permissions(&self, request: &Request<Body>) -> Vec<Permission> {
        let path = request.uri().path();
        let method = match request.method().as_str() {
            // HEAD is GET-without-body (RFC 9110 §9.3.2): same resource read,
            // same permission. Without this, HEAD probes on mapped GET routes
            // (e.g. mero-js getBlobInfo → HEAD /admin-api/blobs/:id) fall into
            // the /admin-api/* default-deny and 403 for scoped client tokens.
            "GET" | "HEAD" => HttpMethod::GET,
            "POST" => HttpMethod::POST,
            "PUT" => HttpMethod::PUT,
            "DELETE" => HttpMethod::DELETE,
            "PATCH" => HttpMethod::PATCH,
            _ => HttpMethod::Any,
        };

        let mut required_permissions = Vec::new();

        // First check exact path matches
        required_permissions.extend(self.get_permissions_for_exact_paths(path, &method));

        // Then check parameterized paths
        required_permissions.extend(get_permissions_for_path_with_params(path, &method));

        // If no matches in path mappings, try the standard resource paths
        if required_permissions.is_empty() {
            let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
            if !components.is_empty() {
                match components[0] {
                    "applications" => {
                        self.add_application_permissions(
                            &components,
                            &method,
                            &mut required_permissions,
                        );
                    }
                    "blobs" => {
                        self.add_blob_permissions(&components, &method, &mut required_permissions);
                    }
                    "contexts" => {
                        self.add_context_permissions(
                            &components,
                            &method,
                            &mut required_permissions,
                        );
                    }
                    "keys" => {
                        self.add_key_permissions(&components, &method, &mut required_permissions);
                    }
                    _ => {}
                }
            }
        }

        // Default-deny for the privileged admin-api namespace.
        //
        // Any `/admin-api/*` route not matched above — an unknown subpath, or a
        // known path reached with an unhandled method (the `_ => vec![]` arms) —
        // produces an empty requirement set. `validate_permissions` treats an
        // empty requirement as a pass (an empty `Iterator::all` is vacuously
        // true), so without this any valid token, including a narrow
        // client-scoped one, would reach the route. Require `admin` instead so
        // unmapped admin routes fail closed.
        //
        // This deliberately makes every unmapped `/admin-api/*` route
        // admin/node-owner only. The client-facing route families —
        // namespaces, group governance, context sub-operations
        // (identities/join/leave/resync/storage/sync), `/admin-api/blobs`,
        // `/admin-api/alias`, `/admin-api/identity/context` — are mapped to
        // scoped permissions above; what remains admin-only is the node
        // operator surface: `install-dev-application`, `usage`,
        // `network/status`, `peers`, `/admin-api/tee/*`,
        // `contexts/invite-specialized-node`, and any future unmapped route.
        // A scoped (non-admin) token that must reach one of these needs an
        // explicit mapping added above. `/jsonrpc`, `/ws`, `/sse` and the
        // public `/auth/*` routes are intentionally outside this namespace
        // and unaffected.
        if required_permissions.is_empty() && path.starts_with("/admin-api/") {
            required_permissions.push(Permission::Admin(AdminPermission));
        }

        required_permissions
    }

    /// Validate that a user has all required permissions
    pub fn validate_permissions(
        &self,
        user_permissions: &[String],
        required_permissions: &[Permission],
    ) -> bool {
        // First check for admin permission
        if user_permissions.iter().any(|p| p == "admin") {
            return true; // Admin has access to everything
        }

        // Convert string permissions to Permission enums for hierarchical checking
        let user_perms: Vec<Permission> = user_permissions
            .iter()
            .filter_map(|p| p.parse::<Permission>().ok())
            .collect();

        // Check if any user permission satisfies each required permission
        required_permissions.iter().all(|req| {
            // Check exact string match first (for simple cases)
            if user_permissions.contains(&req.to_string()) {
                return true;
            }

            // Check hierarchical permissions using satisfies method
            user_perms.iter().any(|user_perm| user_perm.satisfies(req))
        })
    }

    /// Check if a user has a specific permission (string format)
    pub fn has_permission(&self, user_permissions: &[String], permission: &str) -> bool {
        user_permissions.contains(&permission.to_string())
    }

    // Helper methods to add specific types of permissions
    fn add_application_permissions(
        &self,
        components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::{ApplicationPermission, ResourceScope};

        match (method, components.get(1)) {
            (HttpMethod::GET, None) => {
                permissions.push(Permission::Application(ApplicationPermission::List(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::GET, Some(&id)) => {
                permissions.push(Permission::Application(ApplicationPermission::List(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            (HttpMethod::POST, _) => {
                permissions.push(Permission::Application(ApplicationPermission::Install(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::DELETE, Some(&id)) => {
                permissions.push(Permission::Application(ApplicationPermission::Uninstall(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            _ => {}
        }
    }

    fn add_blob_permissions(
        &self,
        components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::{AddBlobPermission, BlobPermission, ResourceScope};

        match (method, components.get(1)) {
            (HttpMethod::POST, Some(&"stream")) => {
                permissions.push(Permission::Blob(BlobPermission::Add(
                    AddBlobPermission::Stream,
                )));
            }
            (HttpMethod::POST, Some(&"file")) => {
                permissions.push(Permission::Blob(BlobPermission::Add(
                    AddBlobPermission::File,
                )));
            }
            (HttpMethod::POST, Some(&"url")) => {
                permissions.push(Permission::Blob(BlobPermission::Add(
                    AddBlobPermission::Url,
                )));
            }
            (HttpMethod::DELETE, Some(&id)) => {
                permissions.push(Permission::Blob(BlobPermission::Remove(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            _ => {}
        }
    }

    fn add_context_permissions(
        &self,
        components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::{ContextPermission, ResourceScope, UserScope};

        match (method, components.get(1), components.get(2)) {
            (HttpMethod::GET, None, None) => {
                permissions.push(Permission::Context(ContextPermission::List(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::GET, Some(&id), None) => {
                permissions.push(Permission::Context(ContextPermission::List(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            (HttpMethod::POST, None, None) => {
                permissions.push(Permission::Context(ContextPermission::Create(
                    ResourceScope::Global,
                )));
            }
            (HttpMethod::DELETE, Some(&id), None) => {
                permissions.push(Permission::Context(ContextPermission::Delete(
                    ResourceScope::Specific(vec![id.to_string()]),
                )));
            }
            (HttpMethod::POST, Some(&id), Some(&"leave")) => {
                permissions.push(Permission::Context(ContextPermission::Leave(
                    ResourceScope::Specific(vec![id.to_string()]),
                    UserScope::Any,
                )));
            }
            (HttpMethod::POST, Some(&id), Some(&"invite")) => {
                permissions.push(Permission::Context(ContextPermission::Invite(
                    ResourceScope::Specific(vec![id.to_string()]),
                    UserScope::Any,
                )));
            }
            (HttpMethod::POST, Some(&id), Some(&"execute")) => {
                permissions.push(Permission::Context(ContextPermission::Execute(
                    ResourceScope::Specific(vec![id.to_string()]),
                    UserScope::Any,
                    None,
                )));
            }
            _ => {}
        }
    }

    fn add_key_permissions(
        &self,
        _components: &[&str],
        method: &HttpMethod,
        permissions: &mut Vec<Permission>,
    ) {
        use super::types::KeyPermission;

        match method {
            HttpMethod::GET => {
                permissions.push(Permission::Keys(KeyPermission::List));
            }
            HttpMethod::POST => {
                permissions.push(Permission::Keys(KeyPermission::Create));
            }
            HttpMethod::DELETE => {
                permissions.push(Permission::Keys(KeyPermission::Delete));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::http::{Method, Request};

    use super::super::types::*;
    use super::*;

    #[test]
    fn test_enhanced_contexts_endpoint() {
        let validator = PermissionValidator::new();

        // Test the new enhanced endpoint
        let req = Request::builder()
            .method(Method::GET)
            .uri("/admin-api/contexts/with-executors/for-application/9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu")
            .body(Body::empty())
            .unwrap();

        let permissions = validator.determine_required_permissions(&req);
        assert_eq!(permissions.len(), 1);
        match &permissions[0] {
            Permission::Context(ContextPermission::List(ResourceScope::Specific(ids))) => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0], "9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu");
            }
            _ => panic!("Unexpected permission type: {:?}", permissions[0]),
        }
    }

    #[test]
    fn test_determine_required_permissions() {
        let validator = PermissionValidator::new();

        // Test admin API application list permission
        let req = Request::builder()
            .method(Method::GET)
            .uri("/admin-api/applications")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        match &perms[0] {
            Permission::Application(ApplicationPermission::List(ResourceScope::Global)) => {}
            _ => panic!("Unexpected permission type"),
        }

        // Test admin API application permission
        let req = Request::builder()
            .method(Method::GET)
            .uri("/admin-api/applications/app1")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        match &perms[0] {
            Permission::Application(ApplicationPermission::List(ResourceScope::Specific(ids))) => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0], "app1");
            }
            _ => panic!("Unexpected permission type"),
        }

        // Test JSON-RPC execute permission (now path-based)
        let req = Request::builder()
            .method(Method::POST)
            .uri("/jsonrpc")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        // JSON-RPC now requires context execute permission
        assert_eq!(perms.len(), 1);
    }

    #[test]
    fn test_validate_permissions() {
        let validator = PermissionValidator::new();

        // Test admin permission
        let user_perms = vec!["admin".to_string()];
        let required = vec![Permission::Application(ApplicationPermission::List(
            ResourceScope::Global,
        ))];
        assert!(validator.validate_permissions(&user_perms, &required));

        // Test specific permission
        let user_perms = vec!["application:list[app1,app2]".to_string()];
        let required = vec![Permission::Application(ApplicationPermission::List(
            ResourceScope::Specific(vec!["app1".to_string()]),
        ))];
        assert!(validator.validate_permissions(&user_perms, &required));

        // Test permission denied
        let user_perms = vec!["blob:add".to_string()];
        let required = vec![Permission::Application(ApplicationPermission::List(
            ResourceScope::Global,
        ))];
        assert!(!validator.validate_permissions(&user_perms, &required));
    }

    #[test]
    fn test_blob_permissions() {
        let validator = PermissionValidator::new();

        // HEAD is a body-less GET: blob info probes (mero-js getBlobInfo)
        // must require blob:get, not fall into the admin default-deny.
        let req = Request::builder()
            .method(Method::HEAD)
            .uri("/admin-api/blobs/blob-1")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        assert!(matches!(
            &perms[0],
            Permission::Blob(BlobPermission::Get(ResourceScope::Specific(ids)))
                if ids == &vec!["blob-1".to_string()]
        ));

        // Test stream upload permission
        let req = Request::builder()
            .method(Method::POST)
            .uri("/blobs/stream")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        assert!(matches!(
            &perms[0],
            Permission::Blob(BlobPermission::Add(AddBlobPermission::Stream))
        ));

        // Test file upload permission
        let req = Request::builder()
            .method(Method::POST)
            .uri("/blobs/file")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        assert!(matches!(
            &perms[0],
            Permission::Blob(BlobPermission::Add(AddBlobPermission::File))
        ));

        // Test URL upload permission
        let req = Request::builder()
            .method(Method::POST)
            .uri("/blobs/url")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        assert!(matches!(
            &perms[0],
            Permission::Blob(BlobPermission::Add(AddBlobPermission::Url))
        ));

        // Test blob deletion permission
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/blobs/123")
            .body(Body::empty())
            .unwrap();
        let perms = validator.determine_required_permissions(&req);
        assert_eq!(perms.len(), 1);
        match &perms[0] {
            Permission::Blob(BlobPermission::Remove(ResourceScope::Specific(ids))) => {
                assert_eq!(ids.len(), 1);
                assert_eq!(ids[0], "123");
            }
            _ => panic!("Unexpected permission type"),
        }

        // Test permission validation
        let validator = PermissionValidator::new();

        // Test that global add permission satisfies specific add permission
        let held = vec!["blob:add".to_string()];
        let required = vec![Permission::Blob(BlobPermission::Add(
            AddBlobPermission::Stream,
        ))];
        assert!(validator.validate_permissions(&held, &required));

        // Test that specific add permission satisfies only that type
        let held = vec!["blob:add:stream".to_string()];
        let required = vec![Permission::Blob(BlobPermission::Add(
            AddBlobPermission::Stream,
        ))];
        assert!(validator.validate_permissions(&held, &required));

        let required = vec![Permission::Blob(BlobPermission::Add(
            AddBlobPermission::File,
        ))];
        assert!(!validator.validate_permissions(&held, &required));

        // Test that global blob permission satisfies everything
        let held = vec!["blob".to_string()];
        let required = vec![
            Permission::Blob(BlobPermission::Add(AddBlobPermission::Stream)),
            Permission::Blob(BlobPermission::Remove(ResourceScope::Specific(vec![
                "123".to_string()
            ]))),
        ];
        assert!(validator.validate_permissions(&held, &required));
    }

    #[test]
    fn test_jsonrpc_permissions() {
        let validator = PermissionValidator::new();

        // Test JSON-RPC execute endpoint
        let request = Request::builder()
            .method(Method::POST)
            .uri("/jsonrpc")
            .body(axum::body::Body::empty())
            .unwrap();

        let permissions = validator.determine_required_permissions(&request);
        assert_eq!(permissions.len(), 1);

        match &permissions[0] {
            Permission::Context(ContextPermission::Execute(scope, user_scope, method)) => {
                assert_eq!(*scope, ResourceScope::Global);
                assert_eq!(*user_scope, UserScope::Any);
                assert_eq!(*method, None);
            }
            _ => panic!("Expected Context Execute permission"),
        }

        // Test permission validation
        let user_permissions = vec!["context:execute".to_string()];
        assert!(validator.validate_permissions(&user_permissions, &permissions));

        let insufficient_permissions = vec!["context:list".to_string()];
        assert!(!validator.validate_permissions(&insufficient_permissions, &permissions));
    }

    #[test]
    fn test_pre_compiled_regex_performance() {
        let validator = PermissionValidator::new();

        // Test various parameterized routes to ensure regex patterns work correctly
        let test_routes = vec![
            ("/admin-api/applications/app-123", Method::GET),
            ("/admin-api/contexts/ctx-456", Method::GET),
            (
                "/admin-api/contexts/ctx-789/capabilities/grant",
                Method::POST,
            ),
            ("/admin-api/contexts/ctx-101/application", Method::POST),
            ("/blobs/blob-123", Method::DELETE),
            ("/admin/keys/key-456", Method::DELETE),
            ("/admin/keys/key-789/permissions", Method::GET),
            ("/admin/keys/key-101/clients/client-123", Method::DELETE),
        ];

        // This test ensures that pre-compiled regexes work correctly
        // In a real performance test, you would measure the time difference
        for (path, method) in test_routes {
            let req = Request::builder()
                .method(method)
                .uri(path)
                .body(Body::empty())
                .unwrap();

            let perms = validator.determine_required_permissions(&req);
            // Each parameterized route should return exactly one permission
            assert!(!perms.is_empty(), "No permissions found for {path}");
        }

        // Note: The performance benefit comes from:
        // 1. LazyLock ensures regexes are compiled only once per pattern
        // 2. No repeated Regex::new() calls on each request
        // 3. Patterns are pre-optimized and cached in memory
        println!("✅ Pre-compiled regex patterns working correctly");
    }

    /// An unmapped `/admin-api/*` route (unknown subpath) must fall back to
    /// requiring `admin`, not to an empty requirement set. This is the
    /// default-deny guard for the audit's "unlisted /admin-api/... subpath →
    /// must be 403, currently 200" finding.
    #[test]
    fn unmapped_admin_api_route_requires_admin() {
        let validator = PermissionValidator::new();

        for (method, path) in [
            (Method::POST, "/admin-api/install-dev-application"),
            (Method::GET, "/admin-api/usage"),
            (Method::GET, "/admin-api/network/status"),
            (Method::GET, "/admin-api/peers"),
            (Method::POST, "/admin-api/contexts/invite-specialized-node"),
            (Method::GET, "/admin-api/totally-unknown-subpath"),
            // Mapped path, unhandled method (the `_ => vec![]` arms):
            (Method::POST, "/admin-api/contexts/ctx-1"),
            (Method::DELETE, "/admin-api/applications"),
            (Method::POST, "/admin-api/blobs/blob-1"),
        ] {
            let req = Request::builder()
                .method(method.clone())
                .uri(path)
                .body(Body::empty())
                .unwrap();
            let required = validator.determine_required_permissions(&req);
            assert_eq!(
                required,
                vec![Permission::Admin(AdminPermission)],
                "{method} {path} must require admin (default-deny), got {required:?}",
            );

            // A narrow, non-admin token must be denied; only admin passes.
            let scoped = vec!["context:execute".to_owned(), "context:list".to_owned()];
            assert!(
                !validator.validate_permissions(&scoped, &required),
                "{method} {path}: a scoped non-admin token must be denied",
            );
            assert!(
                validator.validate_permissions(&["admin".to_owned()], &required),
                "{method} {path}: an admin token must pass",
            );
        }
    }

    /// The default-deny is scoped to `/admin-api/*`. Realtime/public namespaces
    /// (`/jsonrpc` is explicitly mapped; `/ws`, `/sse`, `/auth/*` are not
    /// privileged admin routes) must NOT be forced to admin by the catch-all,
    /// or every app's realtime channel would break.
    #[test]
    fn non_admin_api_namespaces_are_not_force_denied() {
        let validator = PermissionValidator::new();

        // /ws and /sse have no mapping and must stay empty (open to any valid
        // token at the scope gate; their own handlers enforce session/context
        // rules).
        for path in ["/ws", "/sse", "/sse/session/123", "/auth/providers"] {
            let req = Request::builder()
                .method(Method::GET)
                .uri(path)
                .body(Body::empty())
                .unwrap();
            assert!(
                validator.determine_required_permissions(&req).is_empty(),
                "{path} must not be forced to admin by the /admin-api default-deny",
            );
        }

        // /jsonrpc stays mapped to context execute, not admin.
        let req = Request::builder()
            .method(Method::POST)
            .uri("/jsonrpc")
            .body(Body::empty())
            .unwrap();
        let required = validator.determine_required_permissions(&req);
        assert!(matches!(
            required.as_slice(),
            [Permission::Context(ContextPermission::Execute(..))]
        ));
    }

    /// Updating a key's permissions must require `admin`, so a non-admin key
    /// holding a scoped `keys:update-permissions` cannot escalate itself (or
    /// any key) to `admin`. Reading permissions stays a scoped key permission.
    #[test]
    fn updating_key_permissions_requires_admin() {
        let validator = PermissionValidator::new();

        let put = Request::builder()
            .method(Method::PUT)
            .uri("/admin/keys/some-key-id/permissions")
            .body(Body::empty())
            .unwrap();
        let required = validator.determine_required_permissions(&put);
        assert_eq!(required, vec![Permission::Admin(AdminPermission)]);

        // A token scoped only to update-permissions must NOT pass — that is the
        // escalation this guard closes.
        let escalator = vec!["keys:update-permissions[some-key-id]".to_owned()];
        assert!(
            !validator.validate_permissions(&escalator, &required),
            "a non-admin keys:update-permissions token must not be able to update permissions",
        );
        assert!(validator.validate_permissions(&["admin".to_owned()], &required));

        // Reading permissions remains a scoped key permission, not admin-gated.
        let get = Request::builder()
            .method(Method::GET)
            .uri("/admin/keys/some-key-id/permissions")
            .body(Body::empty())
            .unwrap();
        assert!(matches!(
            validator.determine_required_permissions(&get).as_slice(),
            [Permission::Keys(KeyPermission::GetPermissions(_))]
        ));
    }

    /// Creating a root key (`POST /admin/keys`) mints an `["admin"]` key, so it
    /// must require `admin`. A token holding only the strictly-weaker
    /// `keys:create` must NOT be able to mint an admin root key — that is the
    /// privilege escalation this guard closes. Listing keys stays scoped.
    #[test]
    fn creating_root_key_requires_admin() {
        let validator = PermissionValidator::new();

        let post = Request::builder()
            .method(Method::POST)
            .uri("/admin/keys")
            .body(Body::empty())
            .unwrap();
        let required = validator.determine_required_permissions(&post);
        assert_eq!(required, vec![Permission::Admin(AdminPermission)]);

        // A token scoped only to keys:create must be denied.
        let scoped = vec!["keys:create".to_owned()];
        assert!(
            !validator.validate_permissions(&scoped, &required),
            "a non-admin keys:create token must not be able to mint an admin root key",
        );
        assert!(validator.validate_permissions(&["admin".to_owned()], &required));

        // Listing keys remains a scoped key permission, not admin-gated.
        let get = Request::builder()
            .method(Method::GET)
            .uri("/admin/keys")
            .body(Body::empty())
            .unwrap();
        assert!(matches!(
            validator.determine_required_permissions(&get).as_slice(),
            [Permission::Keys(KeyPermission::List)]
        ));
    }

    /// The full grant set a client app token holds (the context trio plus the
    /// namespace/group/blob/alias umbrellas) must reach every client-facing
    /// route family, and must still be denied on the node-operator surface.
    /// This is the post-rc.9 403 regression pin: every route below returned
    /// 403 for app tokens on rc.9/rc.10 because it fell through to the
    /// `/admin-api/*` default-deny.
    #[test]
    fn client_token_battery_covers_client_facing_routes() {
        let validator = PermissionValidator::new();

        let token = [
            "context:create",
            "context:list",
            "context:execute",
            "namespace",
            "group",
            "blob",
            "context:alias",
        ]
        .map(str::to_owned)
        .to_vec();

        for (method, path) in [
            // Namespaces
            (Method::GET, "/admin-api/namespaces"),
            (Method::POST, "/admin-api/namespaces"),
            (Method::GET, "/admin-api/namespaces/ns-1"),
            (Method::DELETE, "/admin-api/namespaces/ns-1"),
            (Method::GET, "/admin-api/namespaces/ns-1/identity"),
            (Method::GET, "/admin-api/namespaces/ns-1/groups"),
            (Method::POST, "/admin-api/namespaces/ns-1/groups"),
            (Method::POST, "/admin-api/namespaces/ns-1/invite"),
            (Method::POST, "/admin-api/namespaces/ns-1/join"),
            (Method::POST, "/admin-api/namespaces/ns-1/leave"),
            (Method::GET, "/admin-api/namespaces/for-application/app-1"),
            // Group governance
            (Method::POST, "/admin-api/groups"),
            (Method::POST, "/admin-api/groups/join"),
            (Method::GET, "/admin-api/groups/grp-1"),
            (Method::PATCH, "/admin-api/groups/grp-1"),
            (Method::DELETE, "/admin-api/groups/grp-1"),
            (Method::GET, "/admin-api/groups/grp-1/members"),
            (Method::POST, "/admin-api/groups/grp-1/members"),
            (Method::PUT, "/admin-api/groups/grp-1/members/id-1/role"),
            (Method::POST, "/admin-api/groups/grp-1/invite"),
            (Method::POST, "/admin-api/groups/grp-1/leave"),
            (Method::GET, "/admin-api/groups/grp-1/contexts"),
            (Method::PUT, "/admin-api/groups/grp-1/metadata"),
            // Context sub-operations
            (Method::GET, "/admin-api/contexts/ctx-1/identities"),
            (Method::GET, "/admin-api/contexts/ctx-1/identities-owned"),
            (Method::GET, "/admin-api/contexts/ctx-1/storage"),
            (Method::GET, "/admin-api/contexts/ctx-1/group"),
            (Method::POST, "/admin-api/contexts/ctx-1/join"),
            (Method::POST, "/admin-api/contexts/ctx-1/leave"),
            (Method::POST, "/admin-api/contexts/ctx-1/resync"),
            (Method::POST, "/admin-api/contexts/sync"),
            (Method::POST, "/admin-api/contexts/sync/ctx-1"),
            // Blobs
            (Method::PUT, "/admin-api/blobs"),
            (Method::GET, "/admin-api/blobs"),
            (Method::GET, "/admin-api/blobs/blob-1"),
            (Method::DELETE, "/admin-api/blobs/blob-1"),
            // Aliases
            (Method::POST, "/admin-api/alias/create/context"),
            (Method::POST, "/admin-api/alias/lookup/context/foo"),
            (Method::POST, "/admin-api/alias/delete/identity/ctx-1/foo"),
            (Method::GET, "/admin-api/alias/list/application"),
            // Identity + RPC
            (Method::POST, "/admin-api/identity/context"),
            (Method::POST, "/jsonrpc"),
        ] {
            let req = Request::builder()
                .method(method.clone())
                .uri(path)
                .body(Body::empty())
                .unwrap();
            let required = validator.determine_required_permissions(&req);
            assert!(
                !required.contains(&Permission::Admin(AdminPermission)),
                "{method} {path} must not require admin, got {required:?}",
            );
            assert!(
                validator.validate_permissions(&token, &required),
                "client token must reach {method} {path} (required: {required:?})",
            );
        }

        // The node-operator surface stays out of reach.
        for (method, path) in [
            (Method::GET, "/admin-api/usage"),
            (Method::POST, "/admin-api/install-dev-application"),
            (Method::GET, "/admin/keys"),
            (Method::PUT, "/admin/keys/key-1/permissions"),
        ] {
            let req = Request::builder()
                .method(method.clone())
                .uri(path)
                .body(Body::empty())
                .unwrap();
            let required = validator.determine_required_permissions(&req);
            assert!(
                !validator.validate_permissions(&token, &required),
                "client token must NOT reach {method} {path}",
            );
        }
    }
}
