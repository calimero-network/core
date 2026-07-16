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

/// The grant set mero-react's companion PR requests once core maps the
/// client-facing admin-api routes: the context trio plus the
/// namespace/group/blob/alias umbrellas. Must be kept in lockstep with
/// `getPermissionsForMode` when that PR lands.
const MULTI_CONTEXT_PERMISSIONS_NEXT: &[&str] = &[
    "context:create",
    "context:list",
    "context:execute",
    "namespace",
    "group",
    "blob",
    "context:alias",
];

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

/// The namespace routes — the *recommended* replacement for direct context
/// creation — are mapped to `namespace:*` permissions. A client token minted
/// with the extended grant set (context trio + umbrellas) must be able to
/// list and create namespaces; the legacy context-only grant set must NOT
/// (the umbrella grants are opt-in, requested at login).
#[test]
fn extended_client_token_can_create_namespaces() {
    let validator = PermissionValidator::new();

    for (method, path) in [
        ("GET", "/admin-api/namespaces"),
        ("POST", "/admin-api/namespaces"),
    ] {
        let required = validator.determine_required_permissions(&request(method, path));
        assert!(
            validator.validate_permissions(&strings(MULTI_CONTEXT_PERMISSIONS_NEXT), &required),
            "an extended client token must be able to call {method} {path} \
             (required: {required:?})",
        );
        assert!(
            !validator.validate_permissions(&strings(MULTI_CONTEXT_PERMISSIONS), &required),
            "the legacy context-only grant set must not reach {method} {path}",
        );
    }
}

/// The extended grant set must also reach every legacy SDK route — adding
/// grants can only widen access, never narrow it.
#[test]
fn extended_client_token_reaches_every_sdk_route() {
    let validator = PermissionValidator::new();

    assert_token_reaches(&validator, &strings(MULTI_CONTEXT_PERMISSIONS_NEXT));
}
