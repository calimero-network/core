use core::convert::Infallible;
use core::fmt::{self, Display, Formatter};
use core::task::{Context, Poll};
use std::error::Error;

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use calimero_store::Store;
use libp2p::futures::future::BoxFuture;
use tower::{Layer, Service};
use tracing::debug;

#[derive(Clone)]
pub struct JwtLayer {
    store: Store,
}

impl JwtLayer {
    pub const fn new(store: Store) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for JwtLayer {
    type Service = JwtMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        JwtMiddleware {
            inner,
            store: self.store.clone(),
        }
    }
}

#[derive(Clone)]
pub struct JwtMiddleware<S> {
    inner: S,
    store: Store,
}

impl<S> Service<Request<Body>> for JwtMiddleware<S>
where
    S: Service<Request<Body>, Response = Response<Body>, Error = Infallible> + Send + 'static,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = Infallible;
    // `BoxFuture` is a type alias for `Pin<Box<dyn Future + Send + 'a>>`
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        // todo! experiment with Interior<Store>: WriteLayer<Interior>
        let result = auth(request.headers(), &self.store.clone());

        if let Err(err) = result {
            let error_response = err.into_response();
            return Box::pin(async move { Ok(error_response) });
        }

        let future = self.inner.call(request);

        Box::pin(async move {
            let response: Response = future.await?;
            Ok(response)
        })
    }
}

#[derive(Debug)]
struct JwtHeader {
    token: String
}

pub fn auth(headers: &HeaderMap, store: &Store) -> Result<(), UnauthorizedError<'static>> {
    let jwt_header = get_jwt_token_from_headers(headers).map_err(|e| {
        debug!("Failed to extract authentication headers {}", e);
        UnauthorizedError::new("Failed to extract authentication headers.")
    })?;

    println!("auth_headers: {:?}", jwt_header.token);
    println!("store: {:?}", store);
    // verification here

    Ok(())
}

fn get_jwt_token_from_headers(headers: &HeaderMap) -> Result<JwtHeader, UnauthorizedError<'_>> {
    let authorization_header = headers
        .get("authorization")
        .ok_or_else(|| UnauthorizedError::new("Missing authorization header"))?;

    let authorization_str = authorization_header
        .to_str()
        .map_err(|_| UnauthorizedError::new("Invalid UTF-8 in authorization header"))?;

    let token = extract_token_from_header(authorization_str)?;

    let auth = JwtHeader { token: token.to_string() };
    Ok(auth)
}

fn extract_token_from_header(authorization_header: &str) -> Result<&str, UnauthorizedError<'_>> {
    let bearer_prefix = "Bearer ";
    if authorization_header.starts_with(bearer_prefix) {
        Ok(&authorization_header[bearer_prefix.len()..])
    } else {
        Err(UnauthorizedError::new("Invalid authorization header"))
    }
}

#[derive(Debug)]
pub struct UnauthorizedError<'a> {
    reason: &'a str,
}

impl<'a> UnauthorizedError<'a> {
    pub const fn new(reason: &'a str) -> Self {
        Self { reason }
    }
}

impl Display for UnauthorizedError<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.pad(self.reason)
    }
}

impl Error for UnauthorizedError<'_> {}

impl IntoResponse for UnauthorizedError<'_> {
    fn into_response(self) -> Response<Body> {
        (StatusCode::UNAUTHORIZED, self.reason.to_owned()).into_response()
    }
}
