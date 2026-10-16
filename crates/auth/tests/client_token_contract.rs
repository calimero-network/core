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

// ---------------------------------------------------------------------------
// Every grant set an app is actually handed today, against every route an app
// actually calls.
//
// The constants above stopped tracking mero-react after its first three
// grants, so nothing here noticed when three hand copies of the MultiContext
// list (the desktop's per-app key, admin-dashboard's, auth-frontend's
// allowlist) were left without `context:delete`: every app got 403 on
// `DELETE /admin-api/contexts/:id` (tauri-app#361, admin-dashboard#188,
// auth-frontend#67). Each copy is pinned verbatim below. When one changes,
// change it here, and the route table says what the change costs.
// ---------------------------------------------------------------------------

/// mero-react `getPermissionsForMode(AppMode.MultiContext)`
/// (`src/context/MeroContext.tsx`), verbatim. auth-frontend forwards it.
const MERO_REACT_MULTI_CONTEXT: &[&str] = &[
    "context:create",
    "context:delete",
    "context:list",
    "context:execute",
    "context:subscribe",
    "application:list",
    "namespace",
    "group",
    "blob",
    "context:alias",
];

/// mero-react `getPermissionsForMode(AppMode.SingleContext)`, verbatim.
const MERO_REACT_SINGLE_CONTEXT: &[&str] = &[
    "context:execute",
    "context:list",
    "context:subscribe",
    "application:list",
    "blob",
    "context:alias",
];

/// tauri-app `APP_TOKEN_PERMISSIONS` (`apps/desktop/src/lib/app-tokens.ts`):
/// the key the desktop mints for each app window.
const DESKTOP_APP_TOKEN: &[&str] = &[
    "context:create",
    "context:delete",
    "context:list",
    "context:execute",
    "context:subscribe",
    "application:list",
    "namespace",
    "group",
    "blob",
    "context:alias",
];

/// admin-dashboard `APP_TOKEN_PERMISSIONS` (`src/utils/openApp.ts`): the key
/// minted for an app opened from the dashboard. Narrower blob verbs on purpose
/// (no node-wide blob listing).
const DASHBOARD_APP_TOKEN: &[&str] = &[
    "context:create",
    "context:delete",
    "context:list",
    "context:execute",
    "context:subscribe",
    "application:list",
    "namespace",
    "group",
    "blob:add",
    "blob:get",
    "blob:remove",
    "context:alias",
];

/// What the desktop minted before tauri-app#361: the MultiContext list
/// without `context:delete`. Kept to pin the 403 it produced.
const DESKTOP_APP_TOKEN_BEFORE_361: &[&str] = &[
    "context:list",
    "context:create",
    "context:execute",
    "context:subscribe",
    "application:list",
    "namespace",
    "group",
    "blob",
    "context:alias",
];

const CTX: &str = "5f4be3609f2916888dcfc0d6568bbcbb2778381cf9669f69738a056ee2617346";

/// The routes a multi-context app calls through mero-js / mero-react after
/// login. `{ctx}` is replaced with [`CTX`].
const MULTI_CONTEXT_APP_ROUTES: &[(&str, &str)] = &[
    ("GET", "/admin-api/contexts"),
    ("POST", "/admin-api/contexts"),
    ("GET", "/admin-api/contexts/{ctx}"),
    ("DELETE", "/admin-api/contexts/{ctx}"),
    ("GET", "/admin-api/contexts/{ctx}/identities-owned"),
    ("POST", "/jsonrpc"),
    ("GET", "/sse"),
    ("POST", "/sse/subscription"),
    ("GET", "/ws"),
    ("GET", "/admin-api/applications"),
    ("GET", "/admin-api/namespaces"),
    ("POST", "/admin-api/namespaces"),
    ("POST", "/admin-api/groups"),
    ("PUT", "/admin-api/blobs"),
    ("POST", "/admin-api/alias/create/context"),
    ("POST", "/admin-api/alias/lookup/context"),
];

/// A single-context app never creates, deletes or governs: it runs, reads,
/// streams and stores files in the context it was opened on.
const SINGLE_CONTEXT_APP_ROUTES: &[(&str, &str)] = &[
    ("GET", "/admin-api/contexts/{ctx}/identities-owned"),
    ("POST", "/jsonrpc"),
    ("GET", "/sse"),
    ("POST", "/sse/subscription"),
    ("GET", "/ws"),
    ("GET", "/admin-api/applications"),
    ("PUT", "/admin-api/blobs"),
    ("POST", "/admin-api/alias/lookup/context"),
];

/// Every route `routes` names that `token` cannot call, as `METHOD path`.
fn refused(
    validator: &PermissionValidator,
    token: &[&str],
    routes: &[(&str, &str)],
) -> Vec<String> {
    let token = strings(token);
    routes
        .iter()
        .map(|(method, path)| (*method, path.replace("{ctx}", CTX)))
        .filter(|(method, path)| {
            let required = validator.determine_required_permissions(&request(method, path));
            !validator.validate_permissions(&token, &required)
        })
        .map(|(method, path)| format!("{method} {path}"))
        .collect()
}

#[test]
fn every_multi_context_grant_set_reaches_every_app_route() {
    let validator = PermissionValidator::new();

    for (source, token) in [
        ("mero-react MultiContext", MERO_REACT_MULTI_CONTEXT),
        ("desktop app token", DESKTOP_APP_TOKEN),
        ("admin-dashboard app token", DASHBOARD_APP_TOKEN),
    ] {
        assert_eq!(
            refused(&validator, token, MULTI_CONTEXT_APP_ROUTES),
            Vec::<String>::new(),
            "{source} {token:?} is refused on routes an app calls",
        );
    }
}

#[test]
fn single_context_grant_set_reaches_every_single_context_route() {
    let validator = PermissionValidator::new();

    assert_eq!(
        refused(
            &validator,
            MERO_REACT_SINGLE_CONTEXT,
            SINGLE_CONTEXT_APP_ROUTES
        ),
        Vec::<String>::new(),
    );
}

/// The copies may differ in shape (the dashboard narrows `blob`) but never in
/// what an app can reach: none may hold a grant mero-react does not ask for
/// beyond a narrower verb of one it does.
#[test]
fn no_app_grant_set_holds_more_than_mero_react_asks_for() {
    let multi: Vec<_> = MERO_REACT_MULTI_CONTEXT
        .iter()
        .map(|p| {
            p.parse::<mero_auth::auth::permissions::Permission>()
                .unwrap()
        })
        .collect();

    for (source, token) in [
        ("desktop app token", DESKTOP_APP_TOKEN),
        ("admin-dashboard app token", DASHBOARD_APP_TOKEN),
    ] {
        for grant in token {
            let held = grant
                .parse::<mero_auth::auth::permissions::Permission>()
                .unwrap_or_else(|e| panic!("{source}: {grant} must parse: {e}"));
            assert!(
                multi.iter().any(|m| m.satisfies(&held)),
                "{source} holds {grant}, which mero-react's MultiContext set does not cover",
            );
        }
    }
}

/// The 403 tauri-app#361 fixed, pinned: a token without `context:delete` is
/// refused deleting a context and nothing else an app calls.
#[test]
fn a_grant_set_without_context_delete_is_refused_only_on_delete() {
    let validator = PermissionValidator::new();

    assert_eq!(
        refused(
            &validator,
            DESKTOP_APP_TOKEN_BEFORE_361,
            MULTI_CONTEXT_APP_ROUTES
        ),
        vec![format!("DELETE /admin-api/contexts/{CTX}")],
    );
}
