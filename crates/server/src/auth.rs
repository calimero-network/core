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
use tracing::info;

use crate::config::ServerConfig;

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
        let (parts, body) = req.into_parts();
        let method = parts.method.clone();
        let headers = parts.headers.clone();

        async move {
            if method != Method::OPTIONS {
                if service.verify_token_from_headers(&headers).await.is_err() {
                    let resp = StatusCode::UNAUTHORIZED.into_response();
                    return Ok(resp);
                }
            }

            let req = Request::from_parts(parts, body);
            let response = inner.call(req).await?;
            Ok(response)
        }
        .boxed()
    }
}
