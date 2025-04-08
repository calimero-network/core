use core::convert::Infallible;
use core::error::Error;
use core::fmt::{self, Display, Formatter};
use core::task::{Context, Poll};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use calimero_store::Store;
use chrono::Utc;
use jsonwebtoken::{decode, DecodingKey, Validation};
use libp2p::futures::future::BoxFuture;
use tower::{Layer, Service};
use tracing::debug;

use crate::admin::storage::jwt_secret::get_jwt_secret;
use crate::admin::utils::jwt::Claims;

#[derive(Clone)]
pub struct JwtLayer {
    store: Store,
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

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        // todo! experiment with Interior<Store>: WriteLayer<Interior>
        let result = auth(req.headers(), &self.store);

        if let Err(err) = result {
            let error_response = err.into_response();
            return Box::pin(async move { Ok(error_response) });
        }

        Box::pin(self.inner.call(req))
    }
}

#[derive(Debug)]
struct JwtHeader {
    token: String,
}

pub fn auth(headers: &HeaderMap, store: &Store) -> Result<(), UnauthorizedError<'static>> {
    let jwt_header = get_jwt_token_from_headers(headers).map_err(|e| {
        debug!("Failed to extract authentication headers {}", e);
        UnauthorizedError::new("Failed to extract authentication headers.")
    })?;

    let jwt_secret = match get_jwt_secret(store) {
        Ok(Some(secret)) => *secret.jwt_secret(),
        Ok(None) => {
            return Err(UnauthorizedError::new("JWT secret not found."));
        }
        Err(_) => {
            return Err(UnauthorizedError::new("Failed to fetch JWT secret."));
        }
    };

    let token_data = decode::<Claims>(
        &jwt_header.token,
        &DecodingKey::from_secret(&jwt_secret),
        &Validation::default(),
    )
    .map_err(|_| UnauthorizedError::new("Token not valid."))?;

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Essentially infallible"
    )]
    let now = Utc::now().timestamp() as usize;
    if token_data.claims.exp < now {
        return Err(UnauthorizedError::new("Token expired."));
    }

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

    let auth = JwtHeader {
        token: token.to_owned(),
    };
    Ok(auth)
}

fn extract_token_from_header(authorization_header: &str) -> Result<&str, UnauthorizedError<'_>> {
    #[expect(clippy::option_if_let_else, reason = "Clearer here")]
    if let Some(token) = authorization_header.strip_prefix("Bearer ") {
        Ok(token)
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
