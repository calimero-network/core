use std::convert::Infallible;
use std::task::{Context, Poll};

use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use calimero_identity::auth::verify_peer_auth;
use calimero_store::Store;
use libp2p::futures::future::BoxFuture;
use libp2p::identity::Keypair;
use tower::{Layer, Service};

use crate::admin::storage::client_keys::get_client_key;

#[derive(Clone)]
pub struct AuthSignatureLayer {
    store: Store,
}

impl AuthSignatureLayer {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}

impl<S> Layer<S> for AuthSignatureLayer {
    type Service = AuthSignatureMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AuthSignatureMiddleware {
            inner,
            keypair: self.keypair.clone(),
        }
    }
}

#[derive(Clone)]
pub struct AuthSignatureMiddleware<S> {
    inner: S,
    keypair: Keypair,
}

impl<S> Service<Request<Body>> for AuthSignatureMiddleware<S>
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
        let result = auth(request.headers(), &self.keypair);

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
struct AuthHeaders {
    client_key: String,
    signature: Vec<u8>,
    challenge: Vec<u8>,
}

pub fn auth<'a>(
    // run the `HeaderMap` extractor
    headers: &'a HeaderMap,
    store: &'a Store,
) -> Result<(), UnauthorizedError<'a>> {
    let auth_headers = get_auth_headers(headers)
        .map_err(|e| UnauthorizedError::new("Failed to extract authentication headers."))?;

    if !exists_client_key(store, &auth_headers.client_key) {
        return Err(UnauthorizedError::new("Client key not found."));
    }

    if verify_client_key(
        auth_headers.client_key.as_str(),
        auth_headers.challenge.as_slice(),
        auth_headers.signature.as_slice(),
    ) {
        Ok(())
    } else {
        Err(UnauthorizedError::new(
            "Invalid signature for provided key.",
        ))
    }

    // match get_auth_headers(&headers) {
    //     Ok(auth_headers) if exists_client_key(store, &auth_headers.client_key) => Ok({
    //         if verify_client_key(
    //             auth_headers.client_key.as_str(),
    //             auth_headers.challenge.as_slice(),
    //             auth_headers.signature.as_slice(),
    //         ) {
    //             Ok(())
    //         } else {
    //             return Err(UnauthorizedError::new("Keypair not matching signature."));
    //         };
    //     }),
    //     Ok(_) => Err(UnauthorizedError::new("Keypair not matching signature.")),
    //     Err(error) => Err(error),
    // }
}

fn get_auth_headers(headers: &HeaderMap) -> Result<AuthHeaders, UnauthorizedError> {
    let client_key = headers
        .get("client_key")
        .ok_or_else(|| UnauthorizedError::new("Missing client_key header"))?;
    let client_key = bs58::decode(client_key)
        .into_vec()
        .map_err(|_| UnauthorizedError::new("Invalid base58 client_key"))?;

    let signature = headers
        .get("signature")
        .ok_or_else(|| UnauthorizedError::new("Missing signature header"))?;
    let signature = bs58::decode(signature)
        .into_vec()
        .map_err(|_| UnauthorizedError::new("Invalid base58 signature"))?;

    let challenge = headers
        .get("challenge")
        .ok_or_else(|| UnauthorizedError::new("Missing challenge header"))?;
    let challenge = bs58::decode(challenge)
        .into_vec()
        .map_err(|_| UnauthorizedError::new("Invalid base58 challenge"))?;

    let auth = AuthHeaders {
        client_key,
        signature,
        challenge,
    };
    Ok(auth)
}

#[derive(Debug)]
pub struct UnauthorizedError<'a> {
    reason: &'a str,
}

impl<'a> UnauthorizedError<'a> {
    pub fn new(reason: &'a str) -> Self {
        Self { reason }
    }
}

impl std::fmt::Display for UnauthorizedError<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(self.reason)
    }
}

impl std::error::Error for UnauthorizedError<'_> {}

impl IntoResponse for UnauthorizedError<'_> {
    fn into_response(self) -> Response<Body> {
        (StatusCode::UNAUTHORIZED, self.reason.to_string()).into_response()
    }
}
