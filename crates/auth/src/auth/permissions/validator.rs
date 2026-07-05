use std::sync::LazyLock;

use axum::body::Body;
use axum::http::Request;
use regex::Regex;

use super::types::{
    AddBlobPermission, AdminPermission, ApplicationPermission, BlobPermission,
    CapabilityPermission, ContextApplicationPermission, ContextPermission, HttpMethod,
    KeyPermission, PackagePermission, Permission, ResourceScope, UserScope,
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

// Namespace routes. Namespaces are containers for contexts, so they map onto
// the same `ContextPermission` set an app already holds — this lets a
// multi-context client token (context:create/list/…) self-serve the
// namespace → group → context flow, while destructive/governance ops
// (delete, invite, leave) map to their restricted equivalents rather than the
// `/admin-api/*` admin-only default-deny. More specific patterns are matched
// before NAMESPACE_REGEX.
static NAMESPACE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)$").unwrap());

static NAMESPACE_GROUPS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)/groups$").unwrap());

static NAMESPACE_IDENTITY_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)/identity$").unwrap());

static NAMESPACE_INVITE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)/invite$").unwrap());

static NAMESPACE_JOIN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)/join$").unwrap());

static NAMESPACE_LEAVE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/([^/]+)/leave$").unwrap());

static NAMESPACES_FOR_APPLICATION_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/namespaces/for-application/([^/]+)$").unwrap());

// Context sub-routes: reads + self-service membership. (capabilities and
// /application are handled by their own regexes above.)
static CONTEXT_SUBROUTE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^/admin-api/contexts/([^/]+)/(join|leave|identities|identities-owned|group|storage|resync)$",
    )
    .unwrap()
});

static CONTEXT_SYNC_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/contexts/sync(?:/([^/]+))?$").unwrap());

// Aliases: create/lookup/delete/list over context|application|identity.
// Friendly names — app-level; writes map to Create, reads to List.
static ALIAS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^/admin-api/alias/(create|lookup|delete|list)/(context|application|identity)(?:/.+)?$",
    )
    .unwrap()
});

// Application versions read.
static APPLICATION_VERSIONS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/applications/([^/]+)/versions$").unwrap());

// Group routes. Most specific first; GROUP_REGEX (bare /groups/:id) last.
static GROUP_MEMBER_SUB_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^/admin-api/groups/([^/]+)/members/([^/]+)/(role|capabilities|auto-follow|metadata)$",
    )
    .unwrap()
});

static GROUP_MEMBERS_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/groups/([^/]+)/members$").unwrap());

static GROUP_MEMBERS_REMOVE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/groups/([^/]+)/members/remove$").unwrap());

static GROUP_CONTEXTS_SUB_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/groups/([^/]+)/contexts/([^/]+)/(metadata|remove)$").unwrap()
});

static GROUP_SETTINGS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^/admin-api/groups/([^/]+)/settings/(default-capabilities|subgroup-visibility|tee-admission-policy)$",
    )
    .unwrap()
});

static GROUP_UPGRADE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/groups/([^/]+)/upgrade(?:/(retry|status))?$").unwrap()
});

static GROUP_STATUS_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/groups/([^/]+)/(cascade-status|migration-status)$").unwrap()
});

static GROUP_MIGRATION_ABORT_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/groups/([^/]+)/migration/abort$").unwrap());

// App-level and read group sub-routes (create/read/join/leave/invite/metadata).
static GROUP_SUBROUTE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^/admin-api/groups/([^/]+)/(contexts|subgroups|metadata|leave|invite|reparent)$")
        .unwrap()
});

// Security-critical group ops kept admin-only (matched so they resolve to
// admin explicitly rather than relying only on the default-deny).
static GROUP_ADMIN_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^/admin-api/groups/([^/]+)/(signing-key|issue-ownership-proof|issue-namespace-ownership-proof)$",
    )
    .unwrap()
});

static GROUP_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^/admin-api/groups/([^/]+)$").unwrap());

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

/// Permission validator for checking request permissions
#[derive(Debug, Default)]
pub struct PermissionValidator;

fn get_permissions_for_path_with_params(path: &str, method: &HttpMethod) -> Vec<Permission> {
    // Handle parameterized routes with pre-compiled regex patterns
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

    if let Some(captures) = APPLICATION_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![app_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => {
                    vec![Permission::Application(ApplicationPermission::List(scope))]
                }
                HttpMethod::DELETE => {
                    vec![Permission::Application(ApplicationPermission::Uninstall(
                        scope,
                    ))]
                }
                _ => vec![],
            };
        }
    }

    // `/contexts/sync[/:id]` must be checked before the generic
    // `/contexts/:id` regex, which would otherwise capture "sync" as a
    // context id.
    if let Some(captures) = CONTEXT_SYNC_REGEX.captures(path) {
        let scope = captures
            .get(1)
            .map(|c| ResourceScope::Specific(vec![c.as_str().to_string()]))
            .unwrap_or(ResourceScope::Global);
        return match method {
            HttpMethod::POST => vec![Permission::Context(ContextPermission::List(scope))],
            _ => vec![],
        };
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

    // Namespace routes (most specific first). Scoped to the namespace id so a
    // token may be narrowed to a namespace; an unscoped context:* token
    // (Global) still satisfies these (Global matches any Specific).
    if let Some(captures) = NAMESPACES_FOR_APPLICATION_REGEX.captures(path) {
        if let Some(app_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![app_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_GROUPS_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Create(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_IDENTITY_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_INVITE_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Invite(
                    scope,
                    UserScope::Any,
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_JOIN_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                // Joining establishes local namespace membership — reachable by
                // the same create authority that lets an app provision its own.
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Create(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_LEAVE_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Leave(
                    scope,
                    UserScope::Any,
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = NAMESPACE_REGEX.captures(path) {
        if let Some(ns_id) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![ns_id.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                HttpMethod::DELETE => vec![Permission::Context(ContextPermission::Delete(scope))],
                _ => vec![],
            };
        }
    }

    // Context sub-routes: reads → List, self-service membership → Create/Leave.
    if let Some(captures) = CONTEXT_SUBROUTE_REGEX.captures(path) {
        if let (Some(ctx_id), Some(action)) = (captures.get(1), captures.get(2)) {
            let scope = ResourceScope::Specific(vec![ctx_id.as_str().to_string()]);
            return match (method, action.as_str()) {
                (HttpMethod::POST, "join") => {
                    vec![Permission::Context(ContextPermission::Create(scope))]
                }
                (HttpMethod::POST, "leave") => vec![Permission::Context(ContextPermission::Leave(
                    scope,
                    UserScope::Any,
                ))],
                // resync refreshes the local replica — a read-family refresh.
                (HttpMethod::POST, "resync") => {
                    vec![Permission::Context(ContextPermission::List(scope))]
                }
                (HttpMethod::GET, "identities" | "identities-owned" | "group" | "storage") => {
                    vec![Permission::Context(ContextPermission::List(scope))]
                }
                _ => vec![],
            };
        }
    }

    // Aliases: friendly names for app resources — low-risk and recreatable.
    // Gated on generic context create/list (not the dedicated `context:alias`
    // variant) so any context-capable client token can name its resources
    // without a fine-grained alias grant. Writes (create/delete) → Create,
    // reads (lookup/list) → List.
    if let Some(captures) = ALIAS_REGEX.captures(path) {
        if let Some(action) = captures.get(1) {
            let scope = ResourceScope::Global;
            return match (method, action.as_str()) {
                (HttpMethod::POST, "create" | "delete") => {
                    vec![Permission::Context(ContextPermission::Create(scope))]
                }
                (HttpMethod::POST, "lookup") | (HttpMethod::GET, "list") => {
                    vec![Permission::Context(ContextPermission::List(scope))]
                }
                _ => vec![],
            };
        }
    }

    // ---- Group routes (most specific first) ----

    // Security-critical: signing keys and ownership proofs stay admin-only.
    if GROUP_ADMIN_REGEX.is_match(path) {
        return match method {
            HttpMethod::POST => vec![Permission::Admin(AdminPermission)],
            _ => vec![],
        };
    }

    // Member governance (add/remove/role/capabilities/auto-follow, member
    // metadata writes) → capability-gated, not a plain client token.
    if let Some(captures) = GROUP_MEMBER_SUB_REGEX.captures(path) {
        if let (Some(gid), Some(action)) = (captures.get(1), captures.get(3)) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match (method, action.as_str()) {
                (HttpMethod::GET, "capabilities" | "metadata") => {
                    vec![Permission::Context(ContextPermission::List(scope))]
                }
                (HttpMethod::PUT, _) => vec![Permission::Context(ContextPermission::Capabilities(
                    CapabilityPermission::Grant(scope),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_MEMBERS_REMOVE_REGEX.captures(path) {
        if let Some(gid) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Capabilities(
                    CapabilityPermission::Revoke(scope),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_MEMBERS_REGEX.captures(path) {
        if let Some(gid) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                // Adding members = granting access → capability-gated.
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Capabilities(
                    CapabilityPermission::Grant(scope),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_CONTEXTS_SUB_REGEX.captures(path) {
        if let (Some(gid), Some(action)) = (captures.get(1), captures.get(3)) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match (method, action.as_str()) {
                (HttpMethod::GET, "metadata") => {
                    vec![Permission::Context(ContextPermission::List(scope))]
                }
                (HttpMethod::PUT, "metadata") => vec![Permission::Context(
                    ContextPermission::Capabilities(CapabilityPermission::Grant(scope)),
                )],
                (HttpMethod::POST, "remove") => vec![Permission::Context(
                    ContextPermission::Capabilities(CapabilityPermission::Revoke(scope)),
                )],
                _ => vec![],
            };
        }
    }

    // TEE admission policy is a trust-boundary setting → admin; other group
    // settings are governance → capability-gated.
    if let Some(captures) = GROUP_SETTINGS_REGEX.captures(path) {
        if let (Some(gid), Some(setting)) = (captures.get(1), captures.get(2)) {
            if setting.as_str() == "tee-admission-policy" {
                return vec![Permission::Admin(AdminPermission)];
            }
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match method {
                HttpMethod::PUT => vec![Permission::Context(ContextPermission::Capabilities(
                    CapabilityPermission::Grant(scope),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_UPGRADE_REGEX.captures(path) {
        if let Some(gid) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            let is_status = captures.get(2).map(|m| m.as_str()) == Some("status");
            return match method {
                // Upgrading the group's app version — app-version management.
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Application(
                    ContextApplicationPermission::Update(scope),
                ))],
                HttpMethod::GET if is_status => {
                    vec![Permission::Context(ContextPermission::List(scope))]
                }
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_STATUS_REGEX.captures(path) {
        if let Some(gid) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_MIGRATION_ABORT_REGEX.captures(path) {
        if let Some(gid) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match method {
                HttpMethod::POST => vec![Permission::Context(ContextPermission::Capabilities(
                    CapabilityPermission::Grant(scope),
                ))],
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_SUBROUTE_REGEX.captures(path) {
        if let (Some(gid), Some(action)) = (captures.get(1), captures.get(2)) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match (method, action.as_str()) {
                (HttpMethod::GET, "contexts" | "subgroups" | "metadata") => {
                    vec![Permission::Context(ContextPermission::List(scope))]
                }
                (HttpMethod::POST, "leave") => vec![Permission::Context(ContextPermission::Leave(
                    scope,
                    UserScope::Any,
                ))],
                (HttpMethod::POST, "invite") => vec![Permission::Context(
                    ContextPermission::Invite(scope, UserScope::Any),
                )],
                // Writing group metadata / restructuring → capability-gated.
                (HttpMethod::PUT, "metadata") | (HttpMethod::POST, "reparent") => {
                    vec![Permission::Context(ContextPermission::Capabilities(
                        CapabilityPermission::Grant(scope),
                    ))]
                }
                _ => vec![],
            };
        }
    }

    if let Some(captures) = GROUP_REGEX.captures(path) {
        if let Some(gid) = captures.get(1) {
            let scope = ResourceScope::Specific(vec![gid.as_str().to_string()]);
            return match method {
                HttpMethod::GET => vec![Permission::Context(ContextPermission::List(scope))],
                HttpMethod::DELETE => vec![Permission::Context(ContextPermission::Delete(scope))],
                // Updating group settings → capability-gated governance.
                HttpMethod::PATCH => vec![Permission::Context(ContextPermission::Capabilities(
                    CapabilityPermission::Grant(scope),
                ))],
                _ => vec![],
            };
        }
    }

    // Aliases and group routes handled above; blobs below.
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

            // Admin API - Namespaces (containers for contexts; mapped onto the
            // context permission set so client tokens can self-serve).
            ("/admin-api/namespaces", HttpMethod::GET) => vec![Permission::Context(
                ContextPermission::List(ResourceScope::Global),
            )],
            ("/admin-api/namespaces", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Create(ResourceScope::Global),
            )],

            // Generate a context identity (prerequisite for join) — app-level.
            ("/admin-api/identity/context", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Create(ResourceScope::Global),
            )],

            // Groups: create / join a group — app-level (like create-in-namespace).
            ("/admin-api/groups", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Create(ResourceScope::Global),
            )],
            ("/admin-api/groups/join", HttpMethod::POST) => vec![Permission::Context(
                ContextPermission::Create(ResourceScope::Global),
            )],

            // Admin auth endpoints
            ("/admin/keys", HttpMethod::GET) => vec![Permission::Keys(KeyPermission::List)],
            ("/admin/keys", HttpMethod::POST) => vec![Permission::Keys(KeyPermission::Create)],
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
            "GET" => HttpMethod::GET,
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
        // admin/node-owner only (governance under `/admin-api/groups`,
        // `/admin-api/alias`, `install-dev-application`, context sub-operations,
        // `/admin-api/blobs`, usage/network/peers, …). A scoped (non-admin)
        // token that must reach one of these needs an explicit mapping added
        // above. `/jsonrpc`, `/ws`, `/sse` and the public `/auth/*` routes are
        // intentionally outside this namespace and unaffected.
        //
        // Liveness/self-check routes are the exception: they carry no
        // authority and must not be forced to admin. `is-authed` exists to
        // report whether the caller's token is valid; `health`/`ready` are
        // probes. They resolve to an empty requirement (any authenticated
        // caller passes). Truly unauthenticated access would require mounting
        // them outside the guard — a separate concern.
        const PUBLIC_ADMIN_API_ROUTES: &[&str] = &[
            "/admin-api/is-authed",
            "/admin-api/health",
            "/admin-api/ready",
        ];
        if required_permissions.is_empty()
            && path.starts_with("/admin-api/")
            && !PUBLIC_ADMIN_API_ROUTES.contains(&path)
        {
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
            // Security-critical group ops stay admin-only even though sibling
            // group routes are now mapped.
            (Method::POST, "/admin-api/groups/some-group-id/signing-key"),
            (Method::POST, "/admin-api/install-dev-application"),
            (Method::GET, "/admin-api/usage"),
            (Method::POST, "/admin-api/tee/attest"),
            (Method::GET, "/admin-api/totally-unknown-subpath"),
            // Mapped path, unhandled method (the `_ => vec![]` arms):
            (Method::POST, "/admin-api/contexts/ctx-1"),
            (Method::DELETE, "/admin-api/applications"),
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
}
