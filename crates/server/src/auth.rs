use std::convert::Infallible;
use std::sync::Arc;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use eyre::Result;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use mero_auth::embedded::{build_app, default_config, EmbeddedAuthApp};
use mero_auth::AuthService;
use tower::{Layer, Service};
use tracing::{debug, info, warn};

use crate::config::ServerConfig;

/// The authenticated requester's public key, injected into request extensions
/// by [`AuthGuardService`] after token verification.
///
/// Handlers extract this via `Extension(AuthenticatedKey(pk))` and use it as
/// the effective requester instead of trusting the value from the request body.
#[derive(Clone, Debug)]
pub struct AuthenticatedKey(pub calimero_primitives::identity::PublicKey);

/// Wrapper around the embedded authentication application, keeping the router and shared state.
pub struct BundledAuth {
    app: EmbeddedAuthApp,
}

impl BundledAuth {
    #[must_use]
    pub fn auth_service(&self) -> AuthService {
        self.app.state.auth_service.clone()
    }

    #[must_use]
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
                                return Ok(StatusCode::UNAUTHORIZED.into_response());
                            }
                        }
                    } else {
                        // No Authorization header — try the ?token= query parameter.
                        // Browser WebSocket and EventSource APIs cannot set custom
                        // headers, so the JS client passes the JWT as a query param.
                        // JWT tokens are base64url-encoded (A-Za-z0-9._-) so no
                        // URL-decoding is required.
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
                                        return Ok(StatusCode::UNAUTHORIZED.into_response());
                                    }
                                }
                            }
                            None => {
                                debug!("No Authorization header and no ?token= query parameter");
                                return Ok(StatusCode::UNAUTHORIZED.into_response());
                            }
                        }
                    };

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
                            Err(err) => {
                                warn!(key_id=%auth_response.key_id, %err, "auth key_id public_key is not a valid PublicKey; skipping extension injection");
                            }
                        }
                    }
                    Ok(None) => {}
                    Err(err) => {
                        warn!(key_id=%auth_response.key_id, %err, "failed to look up public key for auth key_id");
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
