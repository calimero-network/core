use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::OriginalUri;
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use eyre::Result;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use mero_auth::auth::permissions::PermissionValidator;
use mero_auth::embedded::{build_app, default_config, EmbeddedAuthApp};
use mero_auth::{AuthError, AuthService};
use tower::{Layer, Service};
use tracing::{debug, info, warn};

use crate::config::ServerConfig;

/// Build the failure response for a rejected token, keeping the Bearer and
/// query-param paths in sync.
///
/// The status and `X-Auth-Error` hint are chosen by matching the typed
/// [`AuthError`] variant, not by inspecting message text:
/// - [`AuthError::TokenExpired`] → `401` with `token_expired`
/// - [`AuthError::TokenRevoked`] → `403` with `token_revoked`
/// - everything else → bare `401`
///
/// Note that a revoked key often still surfaces as a generic "key not found"
/// because [`KeyManager::get_key`] filters revoked keys out before the
/// `is_valid` check can run; the dedicated arm here ensures that any path which
/// *does* produce [`AuthError::TokenRevoked`] is reported as `403` rather than
/// being collapsed into the generic `401`.
fn unauthorized_response(err: &AuthError) -> Response {
    match err {
        AuthError::TokenExpired => {
            let mut resp = StatusCode::UNAUTHORIZED.into_response();
            resp.headers_mut()
                .insert("X-Auth-Error", HeaderValue::from_static("token_expired"));
            resp
        }
        AuthError::TokenRevoked => {
            let mut resp = StatusCode::FORBIDDEN.into_response();
            resp.headers_mut()
                .insert("X-Auth-Error", HeaderValue::from_static("token_revoked"));
            resp
        }
        _ => StatusCode::UNAUTHORIZED.into_response(),
    }
}

/// The authenticated requester's public key, injected into request extensions
/// by [`AuthGuardService`] after token verification.
///
/// Handlers extract this via `Extension(AuthenticatedKey(pk))` and use it as
/// the effective requester instead of trusting the value from the request body.
#[derive(Clone, Debug)]
pub struct AuthenticatedKey(pub calimero_primitives::identity::PublicKey);

/// Marker injected by [`AuthGuardService`] when a request carries a valid token
/// but the auth method does not produce a cryptographic public key (e.g.
/// embedded username/password). The presence of this extension tells handlers
/// that the caller is the node owner, positively confirmed by the auth layer.
///
/// Using an explicit marker instead of relying on `Option<AuthenticatedKey>`
/// being `None` makes the bypass path auditable: `None` for both extensions
/// means the auth guard did not run (no-auth mode), not that a specific auth
/// method was used. Handlers can match on both extensions and reason about
/// exactly which auth path was taken.
#[derive(Clone, Debug)]
pub struct AuthenticatedNodeOwner;

/// Wrapper around the embedded authentication application, keeping the router and shared state.
pub struct BundledAuth {
    app: EmbeddedAuthApp,
}

impl BundledAuth {
    #[must_use]
    pub fn auth_service(&self) -> AuthService {
        self.app.state.auth_service.clone()
    }

    pub fn into_router(self) -> Router {
        self.app.router
    }
}

/// Initialise the embedded authentication service according to the server configuration.
pub async fn initialise(server_config: &ServerConfig) -> Result<BundledAuth> {
    let auth_config = server_config
        .embedded_auth_config()
        .cloned()
        .unwrap_or_else(default_config);

    // Path resolution is handled by merod run.rs before passing config here
    let app = build_app(auth_config).await?;

    info!("Embedded authentication endpoints enabled at /auth and /admin");

    Ok(BundledAuth { app })
}

#[must_use]
pub fn guard_layer(service: Arc<AuthService>) -> AuthGuardLayer {
    AuthGuardLayer::new(service)
}

#[derive(Clone)]
pub struct AuthGuardLayer {
    service: Arc<AuthService>,
}

impl AuthGuardLayer {
    fn new(service: Arc<AuthService>) -> Self {
        Self { service }
    }
}

impl<S> Layer<S> for AuthGuardLayer
where
    S: Service<Request<Body>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Service = AuthGuardService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthGuardService {
            inner,
            service: Arc::clone(&self.service),
        }
    }
}

#[derive(Clone)]
pub struct AuthGuardService<S> {
    inner: S,
    service: Arc<AuthService>,
}

impl<S> Service<Request<Body>> for AuthGuardService<S>
where
    S: Service<Request<Body>, Response = Response, Error = Infallible> + Clone + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = Response;
    type Error = Infallible;
    type Future = BoxFuture<'static, Result<Response, Infallible>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let mut inner = self.inner.clone();
        let service = Arc::clone(&self.service);
        let (mut parts, body) = req.into_parts();
        let method = parts.method.clone();
        let headers = parts.headers.clone();
        let uri = parts.uri.clone();

        async move {
            if method != Method::OPTIONS {
                let auth_response =
                    if headers.contains_key(axum::http::header::AUTHORIZATION) {
                        // Authorization header is present — validate it exclusively.
                        // Never fall through to the query param path: if the client
                        // explicitly sent a header (even an invalid/revoked one), honour
                        // that choice and reject rather than silently retrying with a
                        // query param token, which would bypass revocation.
                        match service.verify_token_from_headers(&headers).await {
                            Ok(resp) => resp,
                            Err(e) => {
                                debug!(error = ?e, "Bearer token validation failed");
                                return Ok(unauthorized_response(&e));
                            }
                        }
                    } else {
                        // No Authorization header — try the ?token= query parameter.
                        // Browser WebSocket and EventSource APIs cannot set custom
                        // headers, so the JS client passes the JWT as a query param.
                        let token = uri.query().and_then(|q| {
                            q.split('&').find_map(|pair| {
                                let (key, value) = pair.split_once('=')?;
                                (key == "token").then(|| value.to_owned())
                            })
                        });
                        match token {
                            Some(ref t) => {
                                match service.verify_token_string(t, Some(&headers)).await {
                                    Ok(resp) => resp,
                                    Err(e) => {
                                        debug!(error = ?e, "Query param token validation failed");
                                        return Ok(unauthorized_response(&e));
                                    }
                                }
                            }
                            None => {
                                debug!("No Authorization header and no ?token= query parameter");
                                return Ok(StatusCode::UNAUTHORIZED.into_response());
                            }
                        }
                    };

                // Authorisation. Authenticating the token is not enough: a valid
                // but under-privileged token must not reach privileged handlers.
                // Previously this guard stopped at verification, so any valid
                // token could hit every /admin-api/* and /jsonrpc endpoint. Run
                // the same determine + validate pass the auth crate's
                // `auth_middleware` uses, against the permissions carried by the
                // verified token, so the two enforcement paths stay in sync.
                //
                // `PermissionValidator` matches full request paths (e.g.
                // `/admin-api/contexts`, `/jsonrpc`). This guard runs inside a
                // nested router, so `parts.uri` has had the mount prefix stripped
                // (`/contexts`, `/`); recover the full path from `OriginalUri`,
                // which axum's `nest` inserts. Non-nested mounts (e.g. the `/ws`
                // route) have no `OriginalUri`, so fall back to the request URI,
                // which is already the full path there.
                let full_uri = parts
                    .extensions
                    .get::<OriginalUri>()
                    .map_or_else(|| uri.clone(), |original| original.0.clone());

                let perm_request = Request::builder()
                    .method(method.clone())
                    .uri(full_uri)
                    .body(Body::empty())
                    .expect("request built from an already-validated method and URI");

                let validator = PermissionValidator::new();
                let required = validator.determine_required_permissions(&perm_request);
                if !validator.validate_permissions(&auth_response.permissions, &required) {
                    warn!(
                        key_id = %auth_response.key_id,
                        ?required,
                        granted = ?auth_response.permissions,
                        "permission denied: token lacks the permissions this route requires",
                    );
                    let mut resp = StatusCode::FORBIDDEN.into_response();
                    resp.headers_mut()
                        .insert("X-Auth-Error", HeaderValue::from_static("permission_denied"));
                    return Ok(resp);
                }

                // Attempt to resolve the authenticated public key and inject it so
                // handlers can use it as the effective requester without trusting the
                // caller-supplied value.
                match service.get_key_public_key(&auth_response.key_id).await {
                    Ok(Some(pk_hex)) => {
                        use std::str::FromStr as _;
                        match calimero_primitives::identity::PublicKey::from_str(&pk_hex) {
                            Ok(pk) => {
                                parts.extensions.insert(AuthenticatedKey(pk));
                            }
                            Err(_) => {
                                // The stored value is not a valid Ed25519/base58 public
                                // key. This is expected for username/password auth: the
                                // user_password provider stores the username in the
                                // `public_key` field as a human-readable identifier, not
                                // a real cryptographic key. Treat this path identically
                                // to Ok(None) — the auth layer confirmed a valid session;
                                // the caller is the node owner.
                                debug!(key_id=%auth_response.key_id, "non-key auth (parse failure): granting NodeOwner");
                                parts.extensions.insert(AuthenticatedNodeOwner);
                            }
                        }
                    }
                    Ok(None) => {
                        // No Ed25519 public key is stored for this key_id. The only
                        // legitimate case is a client key (`KeyType::Client`): created
                        // via the `/auth/client-keys` API by the node owner for their
                        // own applications and provisioned with `public_key: None` by
                        // design (see `Key::new_client_key`). Client keys are always
                        // issued by and to the node owner; treating them as NodeOwner
                        // matches the intended access model.
                        //
                        // Note: username/password root keys do NOT reach this arm.
                        // The `user_password` provider stores the username as a
                        // non-base58 string in `public_key`, so `get_key_public_key`
                        // returns `Ok(Some(username))` and `PublicKey::from_str` fails
                        // → that path is handled by the `Err(_)` arm above.
                        //
                        // No other key type with `public_key = None` should exist in
                        // the store. This is a schema guarantee in the auth crate:
                        // `new_root_key_with_permissions` always sets `public_key` to
                        // a non-empty string, and `new_client_key` is the only other
                        // constructor.
                        warn!(key_id=%auth_response.key_id, "non-key auth (absent public key): granting NodeOwner");
                        parts.extensions.insert(AuthenticatedNodeOwner);
                    }
                    Err(err) => {
                        // A store or network error during key lookup is an
                        // infrastructure failure, not a known auth path. Fail
                        // closed rather than failing open: do NOT grant
                        // node-owner access on a transient error.
                        warn!(key_id=%auth_response.key_id, %err, "failed to look up public key for auth key_id; rejecting request");
                        return Ok(StatusCode::INTERNAL_SERVER_ERROR.into_response());
                    }
                }
            }

            let req = Request::from_parts(parts, body);
            let response = inner.call(req).await?;
            Ok(response)
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Method, Request};
    use mero_auth::auth::permissions::PermissionValidator;

    /// Build the request the guard hands to the validator: only the method and
    /// the full path are read by `determine_required_permissions`.
    fn request(method: Method, path: &str) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap()
    }

    /// The guard resolves the *full* request path (via `OriginalUri`) precisely
    /// because the validator's mappings are keyed on the server's mount paths.
    /// If these stopped yielding a required permission, enforcement would
    /// silently degrade to "any valid token", which is the bug being fixed.
    #[test]
    fn server_mount_paths_require_permissions() {
        let validator = PermissionValidator::new();

        for (method, path) in [
            (Method::GET, "/admin-api/contexts"),
            (Method::POST, "/admin-api/contexts"),
            (Method::GET, "/admin-api/applications"),
            (Method::POST, "/jsonrpc"),
        ] {
            let required = validator.determine_required_permissions(&request(method.clone(), path));
            assert!(
                !required.is_empty(),
                "{method} {path} must map to a required permission for the guard to enforce it",
            );
        }
    }

    /// End-to-end of the guard's authorisation decision: an authenticated token
    /// with no permissions is rejected for a privileged route, while a token
    /// holding `admin` is allowed.
    #[test]
    fn underprivileged_token_is_denied_admin_token_allowed() {
        let validator = PermissionValidator::new();
        let required =
            validator.determine_required_permissions(&request(Method::GET, "/admin-api/contexts"));

        let no_permissions: Vec<String> = Vec::new();
        assert!(
            !validator.validate_permissions(&no_permissions, &required),
            "a token with no permissions must not pass the contexts-list check",
        );

        let admin = vec!["admin".to_owned()];
        assert!(
            validator.validate_permissions(&admin, &required),
            "an admin token must pass every permission check",
        );
    }

    /// A revoked token must map to `403 Forbidden` with `X-Auth-Error:
    /// token_revoked`. Guards against a silent regression to the generic `401`
    /// if the typed variant is ever removed, renamed, or the arm dropped.
    #[test]
    fn revoked_token_maps_to_forbidden() {
        use axum::http::StatusCode;
        use mero_auth::AuthError;

        let resp = super::unauthorized_response(&AuthError::TokenRevoked);
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert_eq!(resp.headers().get("X-Auth-Error").unwrap(), "token_revoked",);
    }

    /// An expired token maps to `401 Unauthorized` with `X-Auth-Error:
    /// token_expired`.
    #[test]
    fn expired_token_maps_to_unauthorized() {
        use axum::http::StatusCode;
        use mero_auth::AuthError;

        let resp = super::unauthorized_response(&AuthError::TokenExpired);
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(resp.headers().get("X-Auth-Error").unwrap(), "token_expired",);
    }

    /// Any other rejection falls back to a bare `401` with no error hint.
    #[test]
    fn other_errors_map_to_bare_unauthorized() {
        use axum::http::StatusCode;
        use mero_auth::AuthError;

        let resp = super::unauthorized_response(&AuthError::InvalidToken("nope".to_owned()));
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(resp.headers().get("X-Auth-Error").is_none());
    }

    /// Unmapped `/admin-api/*` routes (governance subpaths, unhandled methods)
    /// must fail closed: a valid but non-admin token is denied, an admin token
    /// is allowed. Without the default-deny these admitted any valid token —
    /// the audit's "unlisted /admin-api/... subpath → 403, currently 200".
    #[test]
    fn unmapped_admin_api_routes_deny_non_admin_tokens() {
        let validator = PermissionValidator::new();
        let scoped = vec!["context:execute".to_owned(), "context:list".to_owned()];
        let admin = vec!["admin".to_owned()];

        for (method, path) in [
            (Method::POST, "/admin-api/groups"),
            (Method::DELETE, "/admin-api/groups/g-1"),
            (Method::POST, "/admin-api/install-dev-application"),
            (Method::GET, "/admin-api/usage"),
        ] {
            let required = validator.determine_required_permissions(&request(method.clone(), path));
            assert!(
                !required.is_empty(),
                "{method} {path} must yield a requirement (admin) for the guard to enforce",
            );
            assert!(
                !validator.validate_permissions(&scoped, &required),
                "{method} {path}: a scoped non-admin token must be denied (403)",
            );
            assert!(
                validator.validate_permissions(&admin, &required),
                "{method} {path}: an admin token must be allowed",
            );
        }
    }
}
