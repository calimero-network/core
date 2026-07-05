//! Client-token contract test: the seam between the auth frontends and the
//! permission validator.
//!
//! The rc.9 outage (auth-frontend#33) happened because nothing anywhere
//! asserted that **a token minted with the exact permission strings the
//! frontends send can actually call the routes the SDK uses**. Core's unit
//! tests asserted enforcement, the frontends' tests ran against mocks, and
//! mero-react's e2e authenticated as root admin — every repo green, the
//! product broken.
//!
//! This test pins that seam from core's side:
//!
//! - the permission strings are copied verbatim from their sources:
//!   `mero-react/src/context/MeroContext.tsx` (`getPermissionsForMode`) and
//!   auth-frontend's `LoginView` / `PackageFlow` (which, since
//!   auth-frontend#33, forward them untouched);
//! - the token shape mirrors what `/admin/client-key` mints
//!   (`api/handlers/client_keys.rs`): an optional `context[<ctx>,<identity>]`
//!   binding prepended to the requested permissions;
//! - the routes are the ones the SDK actually calls after login.
//!
//! If a frontend changes what it sends, the matching constant here must be
//! updated in the same breath — that is the point of the pin.

use axum::body::Body;
use axum::http::{Method, Request};
use mero_auth::auth::permissions::PermissionValidator;

/// `getPermissionsForMode(AppMode.MultiContext)` in mero-react — the only
/// non-deprecated app mode. auth-frontend forwards these unmodified.
const MULTI_CONTEXT_PERMISSIONS: &[&str] = &["context:create", "context:list", "context:execute"];

/// The routes a multi-context app depends on after login: the mero-react
/// auth gate used `GET /admin-api/contexts` up to v4.1.0, context
/// self-service needs list + create, and every RPC goes through `/jsonrpc`.
const SDK_ROUTES: &[(&str, &str)] = &[
    ("GET", "/admin-api/contexts"),
    ("POST", "/admin-api/contexts"),
    ("POST", "/jsonrpc"),
];

fn request(method: &str, path: &str) -> Request<Body> {
    Request::builder()
        .method(method.parse::<Method>().unwrap())
        .uri(path)
        .body(Body::empty())
        .unwrap()
}

fn assert_token_reaches(validator: &PermissionValidator, token_permissions: &[String]) {
    for (method, path) in SDK_ROUTES {
        let required = validator.determine_required_permissions(&request(method, path));
        assert!(
            validator.validate_permissions(token_permissions, &required),
            "token {token_permissions:?} must be able to call {method} {path}, \
             but the validator rejected it (required: {required:?})",
        );
    }
}

fn strings(perms: &[&str]) -> Vec<String> {
    perms.iter().map(|p| (*p).to_owned()).collect()
}

/// A multi-context client key is minted with an empty `context_id`, so the
/// handler skips the `context[...]` binding: the token holds exactly the
/// requested permissions. It must reach every SDK route.
#[test]
fn multi_context_client_token_reaches_every_sdk_route() {
    let validator = PermissionValidator::new();

    assert_token_reaches(&validator, &strings(MULTI_CONTEXT_PERMISSIONS));
}

/// When a context is selected at login, `/admin/client-key` prepends the
/// `context[<ctx>,<identity>]` binding. The binding must not cost the token
/// any of the access the bare permission set has.
#[test]
fn context_bound_client_token_reaches_every_sdk_route() {
    let validator = PermissionValidator::new();

    let mut token = vec!["context[ctx-1,member-pk-1]".to_owned()];
    token.extend(strings(MULTI_CONTEXT_PERMISSIONS));

    assert_token_reaches(&validator, &token);
}

/// The session gate (`/auth/validate`, used by mero-react since PR #41) must
/// not require any permission — only a valid token. If a mapping is ever
/// added for it, scoped tokens get locked out of login again.
#[test]
fn auth_validate_requires_no_permissions() {
    let validator = PermissionValidator::new();

    for method in ["GET", "POST"] {
        let required = validator.determine_required_permissions(&request(method, "/auth/validate"));
        assert!(
            required.is_empty(),
            "{method} /auth/validate must require no permissions, got {required:?}",
        );
    }
}

/// Regression pin for the rc.9 outage: permissions scoped to an
/// *application id* (`context:list[<app-id>]`) parse as context-id scopes
/// and can never satisfy the Global requirements of the SDK routes. A
/// frontend reintroducing the app-id rewrite must trip this expectation,
/// not ship another endless login loop.
#[test]
fn app_id_scoped_token_is_rejected_on_every_sdk_route() {
    let validator = PermissionValidator::new();

    let token = strings(&[
        "context:create[9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu]",
        "context:list[9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu]",
        "context:execute[9e4gX24aMx3KWWViZeYu8E4e8UrntWDEsuDTFJTXdKsu]",
    ]);

    for (method, path) in SDK_ROUTES {
        let required = validator.determine_required_permissions(&request(method, path));
        assert!(
            !validator.validate_permissions(&token, &required),
            "app-id-scoped token must be rejected on {method} {path}: this is \
             the rc.9 bug shape and it must never validate",
        );
    }
}

/// The namespace routes are containers for contexts and map onto the same
/// `ContextPermission` set an app already holds, so a multi-context client
/// token can self-serve the `namespace → group → context` flow. This was the
/// gap that 403'd the official app migration path; it is now closed.
#[test]
fn multi_context_client_token_can_self_serve_namespaces() {
    let validator = PermissionValidator::new();
    let token = strings(MULTI_CONTEXT_PERMISSIONS);

    // The full app-driven provisioning chain must be reachable.
    for (method, path) in [
        ("POST", "/admin-api/namespaces"), // create namespace
        ("GET", "/admin-api/namespaces"),  // list namespaces
        ("GET", "/admin-api/namespaces/for-application/app1"), // list for app
        ("GET", "/admin-api/namespaces/ns1"), // get namespace
        ("GET", "/admin-api/namespaces/ns1/groups"), // list groups
        ("POST", "/admin-api/namespaces/ns1/groups"), // create group in namespace
    ] {
        let required = validator.determine_required_permissions(&request(method, path));
        assert!(
            !required.is_empty(),
            "{method} {path} must have an explicit mapping, not fall through to \
             the admin-only default-deny",
        );
        assert!(
            validator.validate_permissions(&token, &required),
            "a multi-context client token must reach {method} {path} \
             (required: {required:?})",
        );
    }
}

/// Least privilege is preserved: destructive namespace deletion is NOT
/// reachable by a plain multi-context token (it needs `context:delete`), so
/// mapping the create/list paths didn't hand apps a demolition tool.
#[test]
fn multi_context_client_token_cannot_delete_a_namespace() {
    let validator = PermissionValidator::new();

    let required =
        validator.determine_required_permissions(&request("DELETE", "/admin-api/namespaces/ns1"));
    assert!(
        !validator.validate_permissions(&strings(MULTI_CONTEXT_PERMISSIONS), &required),
        "namespace deletion must stay gated (required: {required:?})",
    );
    // An admin token still can.
    assert!(
        validator.validate_permissions(&["admin".to_owned()], &required),
        "admin must be able to delete a namespace",
    );
}

// ---- Full admin-api permission map (see crates/auth/docs/admin-api-permissions.md) ----

/// APP routes: reachable by a plain multi-context client token
/// (`context:create/list/execute`). These are the resource CRUD + self-service
/// membership + naming ops an app drives for itself.
#[test]
fn app_routes_are_reachable_by_a_client_token() {
    let validator = PermissionValidator::new();
    let token = strings(MULTI_CONTEXT_PERMISSIONS);

    for (method, path) in [
        // context sub-routes: reads + join + refresh
        ("GET", "/admin-api/contexts/ctx1/identities"),
        ("GET", "/admin-api/contexts/ctx1/group"),
        ("GET", "/admin-api/contexts/ctx1/storage"),
        ("POST", "/admin-api/contexts/ctx1/join"),
        ("POST", "/admin-api/contexts/ctx1/resync"),
        ("POST", "/admin-api/contexts/sync"),
        ("POST", "/admin-api/identity/context"),
        // aliases (create/lookup/list)
        ("POST", "/admin-api/alias/create/context"),
        ("POST", "/admin-api/alias/lookup/context/my-name"),
        ("GET", "/admin-api/alias/list/context"),
        // groups: create / join / read
        ("POST", "/admin-api/groups"),
        ("POST", "/admin-api/groups/join"),
        ("GET", "/admin-api/groups/g1"),
        ("GET", "/admin-api/groups/g1/members"),
        ("GET", "/admin-api/groups/g1/subgroups"),
        ("GET", "/admin-api/groups/g1/upgrade/status"),
    ] {
        let required = validator.determine_required_permissions(&request(method, path));
        assert!(
            !required.is_empty(),
            "{method} {path} must be explicitly mapped, not admin-only default-deny",
        );
        assert!(
            validator.validate_permissions(&token, &required),
            "APP route {method} {path} must be reachable by a client token \
             (required: {required:?})",
        );
    }
}

/// GOV routes: governance over other members. NOT reachable by a plain client
/// token; reachable by one holding `context:capabilities`.
#[test]
fn governance_routes_need_capabilities_not_a_plain_client_token() {
    let validator = PermissionValidator::new();
    let plain = strings(MULTI_CONTEXT_PERMISSIONS);
    let with_caps = strings(&["context:capabilities:grant", "context:capabilities:revoke"]);

    for (method, path) in [
        ("POST", "/admin-api/groups/g1/members"), // add member
        ("POST", "/admin-api/groups/g1/members/remove"), // remove member
        ("PUT", "/admin-api/groups/g1/members/id1/role"), // change role
        ("PUT", "/admin-api/groups/g1/settings/default-capabilities"),
        ("PATCH", "/admin-api/groups/g1"), // group settings
    ] {
        let required = validator.determine_required_permissions(&request(method, path));
        assert!(
            !validator.validate_permissions(&plain, &required),
            "GOV route {method} {path} must NOT be reachable by a plain client token",
        );
        assert!(
            validator.validate_permissions(&with_caps, &required),
            "GOV route {method} {path} must be reachable with context:capabilities \
             (required: {required:?})",
        );
    }
}

/// ADMIN routes: node-owner / security-critical. Only an admin token passes;
/// neither a plain client token nor a capabilities token does.
#[test]
fn admin_routes_stay_admin_only() {
    let validator = PermissionValidator::new();
    let client = strings(MULTI_CONTEXT_PERMISSIONS);
    let caps = strings(&["context:capabilities:grant", "context:capabilities:revoke"]);

    for (method, path) in [
        ("PUT", "/admin-api/groups/g1/settings/tee-admission-policy"),
        ("POST", "/admin-api/groups/g1/signing-key"),
        ("POST", "/admin-api/groups/g1/issue-ownership-proof"),
        ("POST", "/admin-api/install-dev-application"),
        ("POST", "/admin-api/contexts/invite-specialized-node"),
        ("POST", "/admin-api/tee/attest"),
        ("GET", "/admin-api/tee/info"),
        ("GET", "/admin-api/usage"),
        ("GET", "/admin-api/network/status"),
    ] {
        let required = validator.determine_required_permissions(&request(method, path));
        assert!(
            !validator.validate_permissions(&client, &required),
            "ADMIN route {method} {path} must reject a client token",
        );
        assert!(
            !validator.validate_permissions(&caps, &required),
            "ADMIN route {method} {path} must reject a capabilities token",
        );
        assert!(
            validator.validate_permissions(&["admin".to_owned()], &required),
            "ADMIN route {method} {path} must accept admin",
        );
    }
}

/// PUBLIC routes: liveness / token self-check require no permission — any
/// authenticated caller passes (they must NOT hit the admin default-deny).
#[test]
fn public_routes_require_no_permission() {
    let validator = PermissionValidator::new();

    for path in [
        "/admin-api/is-authed",
        "/admin-api/health",
        "/admin-api/ready",
    ] {
        let required = validator.determine_required_permissions(&request("GET", path));
        assert!(
            required.is_empty(),
            "PUBLIC route {path} must require no permission, got {required:?}",
        );
    }
}
